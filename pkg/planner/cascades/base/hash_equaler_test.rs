// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// `hash_equaler` 单元测试。
//
// 对照 Go `hash_equaler_test.go`：验证字符串分段长度不影响哈希边界、
// 同字段不同结构体类型可产生相同摘要但仍不相等，以及各基础类型 Hash* 路径一致。

/// 临时双字符串结构，按字段顺序写入 Hasher。
struct TmpStr {
    str1: String,
    str2: String,
}

impl TmpStr {
    /// 依次 HashString 两个字段，模拟组合对象的哈希写法。
    fn Hash64(&self, h: &mut dyn Hasher) {
        h.HashString(&self.str1);
        h.HashString(&self.str2);
    }
}

/// 验证「abc」+「def」与「abcdef」+空串因长度前缀不同而摘要不同。
#[test]
fn test_string_len() {
    let mut hasher1 = NewHashEqualer();
    let mut hasher2 = NewHashEqualer();
    let a = TmpStr {
        str1: "abc".to_owned(),
        str2: "def".to_owned(),
    };
    let b = TmpStr {
        str1: "abcdef".to_owned(),
        str2: String::new(),
    };
    a.Hash64(hasher1.as_mut());
    b.Hash64(hasher2.as_mut());
    assert_ne!(hasher1.Sum64(), hasher2.Sum64());
}

/// 测试用可哈希且可相等比较的类型擦除接口（对应 Go 接口值）。
trait SX: std::any::Any {
    fn Hash64(&self, h: &mut dyn Hasher);
    fn Equal(&self, sx: &dyn SX) -> bool;
    fn as_any(&self) -> &dyn std::any::Any;
}

/// 结构体类型 A：字段布局与 SB 相同但类型不同。
struct SA {
    a: isize,
    b: String,
}

impl SX for SA {
    fn Hash64(&self, h: &mut dyn Hasher) {
        h.HashInt(self.a);
        h.HashString(&self.b);
    }

    fn Equal(&self, sx: &dyn SX) -> bool {
        // 仅当对端也是 SA 且字段全等时才视为相等。
        sx.as_any()
            .downcast_ref::<SA>()
            .is_some_and(|sa| self.a == sa.a && self.b == sa.b)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// 结构体类型 B：与 SA 字段相同，用于证明哈希相同但类型不等。
struct SB {
    a: isize,
    b: String,
}

impl SX for SB {
    fn Hash64(&self, h: &mut dyn Hasher) {
        h.HashInt(self.a);
        h.HashString(&self.b);
    }

    fn Equal(&self, sx: &dyn SX) -> bool {
        sx.as_any()
            .downcast_ref::<SB>()
            .is_some_and(|sb| self.a == sb.a && self.b == sb.b)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// 同字段不同结构体：摘要可碰撞一致，Equal 仍因类型不同返回 false。
#[test]
fn test_struct_type() {
    let mut hasher1 = NewHashEqualer();
    let mut hasher2 = NewHashEqualer();
    let a = SA {
        a: 1,
        b: "abc".to_owned(),
    };
    let b = SB {
        a: 1,
        b: "abc".to_owned(),
    };
    a.Hash64(hasher1.as_mut());
    b.Hash64(hasher2.as_mut());
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    assert!(!a.Equal(&b));
}

/// 覆盖 HashBool/Int/Int64/Uint64/String/Bytes/Rune 及 Reset 后复用路径。
#[test]
fn test_hash64a() {
    let mut hasher1 = NewHashEqualer();
    let mut hasher2 = NewHashEqualer();

    // 两侧按相同顺序写入相同值，摘要应始终相等。
    hasher1.HashBool(true);
    hasher2.HashBool(true);
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    hasher1.HashBool(false);
    hasher2.HashBool(false);
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    hasher1.HashInt(199);
    hasher2.HashInt(199);
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    hasher1.HashInt64(13_534_523_462_346);
    hasher2.HashInt64(13_534_523_462_346);
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    hasher1.HashUint64(13_534_523_462_346);
    hasher2.HashUint64(13_534_523_462_346);
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    hasher1.HashString("hello");
    hasher2.HashString("hello");
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    hasher1.HashBytes(b"world");
    hasher2.HashBytes(b"world");
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
    for rune in ['我', '是', '谁'] {
        hasher1.HashRune(rune as i32);
        hasher2.HashRune(rune as i32);
    }
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());

    // Reset 后重新哈希字母数字串，确认状态已回到初始 offset。
    hasher1.Reset();
    hasher2.Reset();
    let alphanumeric = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    hasher1.HashString(alphanumeric);
    hasher2.HashString(alphanumeric);
    assert_eq!(hasher1.Sum64(), hasher2.Sum64());
}
