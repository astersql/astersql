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

// 全局连接 ID（GCID）编解码与分配器。
//
// 在开启 GlobalKill（跨集群实例按连接号杀会话）时，连接 ID 需在整个 TiDB 集群唯一。
// 本模块定义 32/64 位位布局、`ParseConnID`/`ToConnID`，以及简单分配器与全局分配器
//（32 位无锁池耗尽后升级到 64 位）。设计见 TiDB global-kill 文档。

use std::sync::atomic::{AtomicI32, Ordering};

use crate::{AutoIncPool, LockFreeCircularPool};

/// 全局连接 ID 逻辑表示：ServerID + 本地连接号 + 是否 64 位编码。
// GCID is the Global Connection ID, providing UNIQUE connection IDs across the whole TiDB cluster.
// Used when GlobalKill feature is enable.
// See https://github.com/pingcap/tidb/blob/master/docs/design/2020-06-01-global-kill.md
// 32 bits version:
// 31 21 20 1 0
//	+--------+------------------+------+
// |serverID| local connID |markup|
// | (11b) | (20b) | =0 |
//	+--------+------------------+------+
// 64 bits version:
// 63 62 41 40 1 0
//	+--+---------------------+--------------------------------------+------+
// | | serverID | local connID |markup|
// |=0| (22b) | (40b) | =1 |
//	+--+---------------------+--------------------------------------+------+
// NOTE:
// 1. `serverId“ in 64 bits version can be less than 2^11. This will happen when the 32 bits local connID has been used up, while `serverID` stay unchanged.
// 2. The local connID of a 32 bits GCID can be the same with another 64 bits GCID. This will not violate the uniqueness of GCID.
// GCID 保留 Go 的三字段布局，Is64bits 决定 ToConnID 采用 32 位还是 64 位编码。
#[derive(Clone, Copy, Debug, Default)]
pub struct GCID {
    /// 实例/服务器编号（写入编码高位）。
    pub ServerID: u64,
    /// 本机局部连接号。
    pub LocalConnID: u64,
    /// 是否按 64 位布局编码（最低位 markup=1）。
    pub Is64bits: bool,
}

/// 32 位布局中 ServerID 位数（默认可被 ldflag 测试改写）。
// ServerIDBits32 is the number of bits of serverID for 32bits global connection ID.
pub static mut ServerIDBits32: u32 = 11;
/// 32 位布局 ServerID 上限。
// MaxServerID32 is maximum serverID for 32bits global connection ID.
pub static mut MaxServerID32: u64 = (1u64 << 11) - 1;
/// 32 位布局中本地连接号位数。
// LocalConnIDBits32 is the number of bits of localConnID for 32bits global connection ID.
pub static mut LocalConnIDBits32: u32 = 20;
/// 32 位布局本地连接号上限。
// MaxLocalConnID32 is maximum localConnID for 32bits global connection ID.
pub static mut MaxLocalConnID32: u64 = (1u64 << 20) - 1;

/// 64 位布局 ServerID 上限（22 位）。
// MaxServerID64 is maximum serverID for 64bits global connection ID.
pub const MaxServerID64: u64 = (1u64 << 22) - 1;
/// 64 位布局本地连接号位数（40）。
// LocalConnIDBits64 is the number of bits of localConnID for 64bits global connection ID.
pub const LocalConnIDBits64: u32 = 40;
/// 64 位布局本地连接号上限。
// MaxLocalConnID64 is maximum localConnID for 64bits global connection ID.
pub const MaxLocalConnID64: u64 = (1u64 << LocalConnIDBits64) - 1;

/// 预留给内部进程的连接号数量（从 ID 空间尾部划出）。
// ReservedCount is the count of reserved connection IDs for internal processes.
pub const ReservedCount: u64 = 200;

impl GCID {
    /// 按 32/64 位布局把字段压成对外可见的 `u64` 连接 ID；越界字段会 panic。
    // ToConnID returns the 64bits connection ID
    // ToConnID 按 Go 的位布局把 ServerID、LocalConnID 和版本标记压成连接 ID。
    pub fn ToConnID(&self) -> u64 {
        let mut id = 0u64;
        if self.Is64bits {
            if self.LocalConnID > MaxLocalConnID64 {
                panic!(
                    "unexpected localConnID {} exceeds {}",
                    self.LocalConnID, MaxLocalConnID64
                );
            }
            if self.ServerID > MaxServerID64 {
                panic!(
                    "unexpected serverID {} exceeds {}",
                    self.ServerID, MaxServerID64
                );
            }

            id |= 0x1;
            id |= self.LocalConnID << 1; // 40 bits local connID.
            id |= self.ServerID << 41; // 22 bits serverID.
        } else {
            unsafe {
                let max_local_conn_id32 = MaxLocalConnID32;
                let max_server_id32 = MaxServerID32;
                if self.LocalConnID > max_local_conn_id32 {
                    panic!(
                        "unexpected localConnID {} exceeds {}",
                        self.LocalConnID, max_local_conn_id32
                    );
                }
                if self.ServerID > max_server_id32 {
                    panic!(
                        "unexpected serverID {} exceeds {}",
                        self.ServerID, max_server_id32
                    );
                }

                id |= self.LocalConnID << 1; // 20 bits local connID.
                id |= self.ServerID << 21; // 11 bits serverID.
            }
        }
        id
    }
}

/// 将 `u64` 连接 ID 解析为 `GCID`；返回值中的 `bool` 表示旧客户端是否截断了 64 位 ID。
// ParseConnID parses an uint64 connection ID to GlobalConnID.
//	`isTruncated` indicates that older versions of the client truncated the 64-bit GlobalConnID to 32-bit.
// ParseConnID 保留 Go 的三返回值语义：解析结果、是否被旧客户端截断、错误。
pub fn ParseConnID(id: u64) -> Result<(GCID, bool), String> {
    if id & 0x80000000_00000000 > 0 {
        return Err("unexpected connectionID exceeds int64".to_string());
    }
    if id & 0x1 > 0 {
        // 64bits
        if id & 0xffffffff_00000000 == 0 {
            // 低 32 位看起来像被旧客户端截断的 64 位 ID，Go 返回空 GCID 和 isTruncated=true。
            return Ok((GCID::default(), true));
        }
        return Ok((
            GCID {
                Is64bits: true,
                LocalConnID: (id >> 1) & MaxLocalConnID64,
                ServerID: (id >> 41) & MaxServerID64,
            },
            false,
        ));
    }

    // 32bits
    if id & 0xffffffff_00000000 > 0 {
        return Err("unexpected connectionID exceeds uint32".to_string());
    }
    unsafe {
        Ok((
            GCID {
                Is64bits: false,
                LocalConnID: (id >> 1) & MaxLocalConnID32,
                ServerID: (id >> 21) & MaxServerID32,
            },
            false,
        ))
    }
}

///////////////////////////////// Class Diagram ///////////////////////////////////
//                                                                               //
// +----------+ +-----------------+ +-----------------------+ //
// | Server | ---> | ConnIDAllocator | <<--+-- | GlobalConnIDAllocator | --+ //
// +----------+ +-----------------+ | +-----------------------+ | //
// +-- | SimpleConnIDAllocator | | //
// +----------+------------+ | //
// | | //
// V | //
// +--------+ +----------------------+ | //
// | IDPool | <<--+-- | AutoIncPool | <--+ //
// +--------+ | +----------------------+ | //
// +-- | LockFreeCircularPool | <--+ //
// +----------------------+ //
//                                                                               //
///////////////////////////////////////////////////////////////////////////////////

/// 获取当前实例 ServerID 的回调类型；与 Go `func() uint64` 一样可捕获状态。
pub type serverIDGetterFn = Box<dyn Fn() -> u64 + Send + Sync + 'static>;

/// 连接 ID 分配器接口：申请、释放、取保留号。
// Allocator allocates global connection IDs.
// Allocator trait 机械对应 Go interface，保留 NextID/Release/GetReservedConnID 三个方法。
pub trait Allocator {
    /// 分配下一个连接 ID（已编码为 `u64`）。
    // NextID returns next connection ID.
    fn NextID(&self) -> u64;
    /// 将连接 ID 归还给分配器/池。
    // Release releases connection ID to allocator.
    fn Release(&self, connectionID: u64);
    /// 按序号取内部保留连接 ID（`reservedNo < ReservedCount`）。
    // GetReservedConnID returns reserved connection ID.
    fn GetReservedConnID(&self, reservedNo: u64) -> u64;
}

/// 未开启 GlobalKill 时的简单自增分配器。
// SimpleAllocator is a simple connection id allocator used when GlobalKill feature is disable.
// SimpleAllocator 使用 AutoIncPool 顺序分配连接 ID，对应未开启 GlobalKill 的简单模式。
pub struct SimpleAllocator {
    /// 普通连接 ID 自增池（尾部 ReservedCount 已从容量中扣除）。
    pool: AutoIncPool,
}

/// 构造简单分配器并初始化池容量。
// NewSimpleAllocator creates a new SimpleAllocator.
pub fn NewSimpleAllocator() -> SimpleAllocator {
    let mut a = SimpleAllocator {
        pool: AutoIncPool::default(),
    };
    // Go 将 MaxUint64-ReservedCount 作为普通连接 ID 池容量，尾部 ReservedCount 留给内部连接。
    a.pool.Init(u64::MAX - ReservedCount);
    a
}

impl Allocator for SimpleAllocator {
    // NextID implements ConnIDAllocator interface.
    fn NextID(&self) -> u64 {
        let (id, _) = self.pool.Get();
        id
    }

    // Release implements ConnIDAllocator interface.
    fn Release(&self, id: u64) {
        self.pool.Put(id);
    }

    // GetReservedConnID implements ConnIDAllocator interface.
    fn GetReservedConnID(&self, reservedNo: u64) -> u64 {
        if reservedNo >= ReservedCount {
            panic!("invalid reservedNo exceed ReservedCount");
        }
        u64::MAX - reservedNo
    }
}

/// 集群级全局连接 ID 分配器：32 位池 + 64 位池，可升级/降级。
// GlobalAllocator is global connection ID allocator.
// GlobalAllocator 组合 32 位无锁环形池和 64 位自增池，并用原子标记记录当前分配模式。
pub struct GlobalAllocator {
    /// 非 0 表示当前按 64 位分配。
    is64bits: AtomicI32, // !0: true, 0: false
    /// 读取本实例 ServerID。
    serverIDGetter: serverIDGetterFn,

    /// 32 位本地连接号无锁环形池。
    local32: LockFreeCircularPool,
    /// 64 位本地连接号自增池。
    local64: AutoIncPool,
}

impl GlobalAllocator {
    /// 当前是否处于 64 位分配模式。
    // is64 indicates allocate 64bits global connection ID or not.
    pub fn is64(&self) -> bool {
        self.is64bits.load(Ordering::SeqCst) != 0
    }

    /// 升级到 64 位分配模式（通常因 32 位池耗尽）。
    // upgradeTo64 upgrade allocator to 64bits.
    pub fn upgradeTo64(&self) {
        self.is64bits.store(1, Ordering::SeqCst);
        log::info!("GlobalAllocator upgrade to 64 bits");
    }

    /// 降级回 32 位分配模式（32 位池空闲回落后触发）。
    pub fn downgradeTo32(&self) {
        self.is64bits.store(0, Ordering::SeqCst);
        log::info!("GlobalAllocator downgrade to 32 bits");
    }

    /// 创建全局分配器；`enable32Bits=false` 则直接从 64 位模式开始。
    // NewGlobalAllocator creates a GlobalAllocator.
    pub fn NewGlobalAllocator<F>(serverIDGetter: F, enable32Bits: bool) -> GlobalAllocator
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        let mut g = GlobalAllocator {
            is64bits: AtomicI32::new(0),
            serverIDGetter: Box::new(serverIDGetter),
            local32: LockFreeCircularPool::default(),
            local64: AutoIncPool::default(),
        };
        g.local32
            .InitExt(1 << unsafe { LocalConnIDBits32 }, u32::MAX);
        g.local64.InitExt(
            (1u64 << LocalConnIDBits64) - ReservedCount,
            true,
            LocalConnIDAllocator64TryCount,
        );

        let is64 = if enable32Bits { 0 } else { 1 };
        g.is64bits.store(is64, Ordering::SeqCst);
        g
    }

    /// 分配一个逻辑 `GCID`（优先 32 位，失败则升级并走 64 位）。
    // Allocate allocates a new global connection ID.
    pub fn Allocate(&self) -> GCID {
        let serverID = (self.serverIDGetter)();

        // 32bits.
        unsafe {
            if !self.is64() && serverID <= MaxServerID32 {
                let (localConnID, ok) = self.local32.Get();
                if ok {
                    return GCID {
                        ServerID: serverID,
                        LocalConnID: localConnID,
                        Is64bits: false,
                    };
                }
                // 32 位本地 ID 池耗尽时切到 64 位模式，和 Go 的 “go on to 64bits” 分支一致。
                self.upgradeTo64();
            }
        }

        // 64bits.
        let (localConnID, ok) = self.local64.Get();
        if !ok {
            // local connID with 40bits pool size is big enough and should not be exhausted, as `MaxServerConnections` is no more than math.MaxUint32.
            panic!(
                "Failed to allocate 64bits local connID after try {} times. Should never happen",
                LocalConnIDAllocator64TryCount
            );
        }
        GCID {
            ServerID: serverID,
            LocalConnID: localConnID,
            Is64bits: true,
        }
    }
}

impl Allocator for GlobalAllocator {
    // NextID returns next connection ID.
    fn NextID(&self) -> u64 {
        let globalConnID = self.Allocate();
        globalConnID.ToConnID()
    }

    // GetReservedConnID implements ConnIDAllocator interface.
    fn GetReservedConnID(&self, reservedNo: u64) -> u64 {
        if reservedNo >= ReservedCount {
            panic!("invalid reservedNo exceed ReservedCount");
        }

        let serverID = (self.serverIDGetter)();
        let globalConnID = GCID {
            ServerID: serverID,
            LocalConnID: (1u64 << LocalConnIDBits64) - 1 - reservedNo,
            Is64bits: true,
        };
        globalConnID.ToConnID()
    }

    // Release releases connectionID to pool.
    fn Release(&self, connectionID: u64) {
        let (globalConnID, isTruncated) = match ParseConnID(connectionID) {
            Ok(v) => v,
            Err(err) => {
                log::error!(
                    "failed to ParseGlobalConnID: error={err}, connectionID={connectionID}, isTruncated=false"
                );
                return;
            }
        };
        if isTruncated {
            log::error!(
                "failed to ParseGlobalConnID: connectionID={connectionID}, isTruncated={isTruncated}"
            );
            return;
        }

        if globalConnID.Is64bits {
            self.local64.Put(globalConnID.LocalConnID);
        } else {
            let ok = self.local32.Put(globalConnID.LocalConnID);
            if ok {
                if self.local32.Len() < self.local32.Cap() / 2 {
                    // 32 位池空闲量回落到一半以下时，Go 会降级回 32 位分配模式。
                    self.downgradeTo32();
                }
            } else {
                log::error!(
                    "failed to release 32bits connection ID: connectionID={connectionID}, localConnID={}",
                    globalConnID.LocalConnID
                );
            }
        }
    }
}

/// 64 位本地 ID 自增池在冲突时的最大尝试次数。
// LocalConnIDAllocator64TryCount is the try count of 64bits local connID allocation.
pub const LocalConnIDAllocator64TryCount: i32 = 10;

/// 链接期/测试开关：是否启用缩小位数的 global-kill 测试布局。
pub static mut ldflagIsGlobalKillTest: &str = "0"; // 1:Yes, otherwise:No.
/// 测试用 ServerID32 位数（字符串，解析失败则 panic）。
pub static mut ldflagServerIDBits32: &str = "11"; // Bits of ServerID32.
/// 测试用 LocalConnID32 位数。
pub static mut ldflagLocalConnIDBits32: &str = "20"; // Bits of LocalConnID32.

/// 按 ldflag 改写 32 位布局常量；仅当测试开关为 `"1"` 时生效。
// initByLDFlagsForGlobalKill 对应 Go init 前调用的 ldflag 解析逻辑，仅在测试开关为 1 时改写 32 位布局。
pub fn initByLDFlagsForGlobalKill() {
    unsafe {
        if ldflagIsGlobalKillTest == "1" {
            let server_id_bits_flag = ldflagServerIDBits32;
            let local_conn_id_bits_flag = ldflagLocalConnIDBits32;
            let i = server_id_bits_flag
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("invalid ldflagServerIDBits32"));
            ServerIDBits32 = i;
            MaxServerID32 = (1u64 << ServerIDBits32) - 1;

            let i = local_conn_id_bits_flag
                .parse::<u32>()
                .unwrap_or_else(|_| panic!("invalid ldflagLocalConnIDBits32"));
            LocalConnIDBits32 = i;
            MaxLocalConnID32 = (1u64 << LocalConnIDBits32) - 1;

            let server_id_bits32 = ServerIDBits32;
            let max_server_id32 = MaxServerID32;
            let local_conn_id_bits32 = LocalConnIDBits32;
            let max_local_conn_id32 = MaxLocalConnID32;
            log::info!(
                "global_kill_test is enabled: ServerIDBits32={}, MaxServerID32={}, LocalConnIDBits32={}, MaxLocalConnID32={}",
                server_id_bits32,
                max_server_id32,
                local_conn_id_bits32,
                max_local_conn_id32
            );
        }
    }
}

/// 包初始化入口：解析 global-kill 相关 ldflag。
// Go 的 init 会在包加载时自动执行；保留同名入口，后续接线时可由模块初始化逻辑显式调用。
pub fn init() {
    initByLDFlagsForGlobalKill();
}
