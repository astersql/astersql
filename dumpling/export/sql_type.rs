// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

// 列类型 → RowReceiver 映射与 SQL 字面量转义，对应 Go `sql_type.go`。
//
// 扫描结果经 `RowReceiverStringer` 格式化为 INSERT 行；数值/字符串/二进制
// 三类 receiver 与 MySQL SHOW COLUMNS 类型名对齐。转义规则遵循 mysqldump 的
// NO_BACKSLASH_ESCAPES 开关语义。

/// 列类型名 → receiver 工厂；懒初始化，与 Go `colTypeRowReceiverMap` 等价。
static COL_TYPE_MAP: OnceLock<HashMap<&'static str, fn() -> Box<dyn crate::RowReceiverStringer>>> =
    OnceLock::new();
/// 纯字符串类型集合，供 `pickupPossibleField` 等排除非数值索引。
static DATA_TYPE_STRING: OnceLock<HashSet<&'static str>> = OnceLock::new();
// 整数类型集合，不单独映射 receiver。
static DATA_TYPE_NUM: OnceLock<HashSet<&'static str>> = OnceLock::new();
static DATA_TYPE_INT: OnceLock<HashSet<&'static str>> = OnceLock::new();
// 二进制/几何类型集合。
static DATA_TYPE_BIN: OnceLock<HashSet<&'static str>> = OnceLock::new();

/// SQL 文本中的 NULL 字面量；Go `nullValue`。
const NULL_VALUE: &str = "NULL";

/// 填充三张类型表与 COL_TYPE_MAP；进程内只执行一次。
pub fn initColTypeRowReceiverMap() {
    let _ = COL_TYPE_MAP.get_or_init(|| {
        let mut map: HashMap<&'static str, fn() -> Box<dyn crate::RowReceiverStringer>> =
            HashMap::new();
        let mut ds = HashSet::new();
        let mut di = HashSet::new();
        let mut db = HashSet::new();
        let mut dn = HashSet::new();

        // 与 Go dataTypeStringArr 一致：引号包裹、日期时间、JSON/ENUM 等走字符串 receiver。
        let data_type_string_arr = [
            "CHAR",
            "NCHAR",
            "VARCHAR",
            "NVARCHAR",
            "CHARACTER",
            "VARCHARACTER",
            "TIMESTAMP",
            "DATETIME",
            "DATE",
            "TIME",
            "YEAR",
            "SQL_TSI_YEAR",
            "TEXT",
            "TINYTEXT",
            "MEDIUMTEXT",
            "LONGTEXT",
            "ENUM",
            "SET",
            "JSON",
            "NULL",
            "VAR_STRING",
        ];
        let data_type_int_arr = [
            "INTEGER",
            "BIGINT",
            "TINYINT",
            "SMALLINT",
            "MEDIUMINT",
            "INT",
            "INT1",
            "INT2",
            "INT3",
            "INT8",
            "UNSIGNED INT",
            "UNSIGNED BIGINT",
            "UNSIGNED TINYINT",
            "UNSIGNED SMALLINT",
        ];
        // 数值类型含浮点/布尔；映射到 SQLTypeNumber（无引号输出）。
        let data_type_num_arr = [
            "INTEGER",
            "BIGINT",
            "TINYINT",
            "SMALLINT",
            "MEDIUMINT",
            "INT",
            "INT1",
            "INT2",
            "INT3",
            "INT8",
            "UNSIGNED INT",
            "UNSIGNED BIGINT",
            "UNSIGNED TINYINT",
            "UNSIGNED SMALLINT",
            "FLOAT",
            "REAL",
            "DOUBLE",
            "DOUBLE PRECISION",
            "DECIMAL",
            "NUMERIC",
            "FIXED",
            "BOOL",
            "BOOLEAN",
        ];
        let data_type_bin_arr = [
            "BLOB",
            "TINYBLOB",
            "MEDIUMBLOB",
            "LONGBLOB",
            "LONG",
            "BINARY",
            "VARBINARY",
            "BIT",
            "GEOMETRY",
        ];
        // 字符串类型注册 SQLTypeStringMaker。
        for s in data_type_string_arr {
            ds.insert(s);
            map.insert(s, SQLTypeStringMaker);
        }
        // 整数集合仅用于 dataTypeIntContains。
        for s in data_type_int_arr {
            di.insert(s);
        }
        // 数值（含浮点/布尔）走 SQLTypeNumberMaker。
        for s in data_type_num_arr {
            dn.insert(s);
            map.insert(s, SQLTypeNumberMaker);
        }
        // BLOB/BIT 等走十六进制 SQLTypeBytesMaker。
        for s in data_type_bin_arr {
            db.insert(s);
            map.insert(s, SQLTypeBytesMaker);
        }
        // OnceLock 仅 set 一次。
        let _ = DATA_TYPE_STRING.set(ds);
        let _ = DATA_TYPE_INT.set(di);
        let _ = DATA_TYPE_BIN.set(db);
        let _ = DATA_TYPE_NUM.set(dn);
        map
    });
}

/// 判断列类型是否归类为字符串；Go `dataTypeStringContains`。
pub fn dataTypeStringContains(s: &str) -> bool {
    // 确保 lazy map 已构建。
    initColTypeRowReceiverMap();
    DATA_TYPE_STRING
        .get()
        .map(|m| m.contains(s))
        .unwrap_or(false)
}
/// 判断是否为整数索引候选类型；Go `dataTypeIntContains`。
pub fn dataTypeIntContains(s: &str) -> bool {
    initColTypeRowReceiverMap();
    DATA_TYPE_INT.get().map(|m| m.contains(s)).unwrap_or(false)
}
pub fn dataTypeNumContains(s: &str) -> bool {
    initColTypeRowReceiverMap();
    DATA_TYPE_NUM.get().is_some_and(|m| m.contains(s))
}

/// 判断是否为二进制/几何类型；Go `dataTypeBinContains`。
pub fn dataTypeBinContains(s: &str) -> bool {
    initColTypeRowReceiverMap();
    DATA_TYPE_BIN.get().map(|m| m.contains(s)).unwrap_or(false)
}

/// MySQL 反斜杠转义（SQL 模式）；Go `escapeBackslashSQL`。
///
/// 转义 NUL、换行、引号、反斜杠及 0x1a（Ctrl+Z）；其余字节原样复制。
pub fn escapeBackslashSQL(s: &[u8], bf: &mut Vec<u8>) {
    let mut last = 0usize;
    for (i, &b) in s.iter().enumerate() {
        let escape = match b {
            // NUL → \0。
            0 => Some(b'0'),
            // 换行 → \n。
            b'\n' => Some(b'n'),
            // 回车 → \r。
            b'\r' => Some(b'r'),
            // 反斜杠加倍。
            b'\\' => Some(b'\\'),
            // 单引号转义。
            b'\'' => Some(b'\''),
            // 双引号转义（SQL 模式）。
            b'"' => Some(b'"'),
            // Ctrl+Z → \Z，MySQL 传统转义。
            0x1a => Some(b'Z'),
            _ => None,
        };
        if let Some(esc) = escape {
            // 复制未转义前缀段。
            bf.extend_from_slice(&s[last..i]);
            // 写入转义引导反斜杠。
            bf.push(b'\\');
            // 写入转义后的代表字符。
            bf.push(esc);
            // 推进未处理起点。
            last = i + 1;
        }
    }
    // 追加尾部未转义字节。
    bf.extend_from_slice(&s[last..]);
}

/// SQL 字面量转义入口；`escape_backslash` 对应会话 NO_BACKSLASH_ESCAPES 取反。
pub fn escapeSQL(s: &[u8], bf: &mut Vec<u8>, escape_backslash: bool) {
    if escape_backslash {
        escapeBackslashSQL(s, bf);
    } else {
        // 标准 SQL 模式：单引号加倍即可。
        for &b in s {
            // 非 backslash 模式：单引号加倍。
            if b == b'\'' {
                // 字符串 SQL 字面量引号。
                bf.push(b'\'');
                bf.push(b'\'');
            } else {
                bf.push(b);
            }
        }
    }
}

/// 字符串列 receiver 工厂。
pub fn SQLTypeStringMaker() -> Box<dyn crate::RowReceiverStringer> {
    Box::new(SQLTypeString {
        raw: RawBytes(None),
    })
}
/// 二进制列 receiver 工厂。
pub fn SQLTypeBytesMaker() -> Box<dyn crate::RowReceiverStringer> {
    Box::new(SQLTypeBytes {
        raw: RawBytes(None),
    })
}
/// 数值列 receiver 工厂（包装 SQLTypeString 复用 BindAddress）。
pub fn SQLTypeNumberMaker() -> Box<dyn crate::RowReceiverStringer> {
    Box::new(SQLTypeNumber {
        inner: SQLTypeString {
            raw: RawBytes(None),
        },
    })
}

/// 按 SHOW COLUMNS 类型名构造每列 receiver 数组；未知类型回退字符串。
pub fn MakeRowReceiver(col_types: &[String]) -> RowReceiverArr {
    initColTypeRowReceiverMap();
    let map = COL_TYPE_MAP.get().unwrap();
    let mut receivers = Vec::with_capacity(col_types.len());
    for col_tp in col_types {
        // 未知列类型回退字符串。
        let maker = map
            .get(col_tp.as_str())
            .copied()
            .unwrap_or(SQLTypeStringMaker);
        // 每列独立 receiver 实例。
        receivers.push(maker());
    }
    RowReceiverArr {
        bound: false,
        receivers,
    }
}

/// 一行多列 receiver 的容器；实现 `RowReceiver`/`Stringer` 组合 trait。
pub struct RowReceiverArr {
    pub bound: bool,
    pub receivers: Vec<Box<dyn crate::RowReceiverStringer>>,
}

impl crate::RowReceiver for RowReceiverArr {
    fn BindAddress(&mut self, args: &mut [RawBytes]) {
        // Always refresh receiver buffers from the latest scanned args.
        // 每列绑定扫描缓冲区切片；Go 同样逐列 BindAddress。
        // BindAddress 成功标记。
        self.bound = true;
        for i in 0..args.len() {
            // 每列绑定独立 RawBytes 切片。
            self.receivers[i].BindAddress(&mut args[i..i + 1]);
        }
    }
}
impl crate::Stringer for RowReceiverArr {
    /// SQL INSERT 行格式：`(v1,v2,...)`。
    fn WriteToBuffer(&self, bf: &mut Vec<u8>, escape_backslash: bool) {
        // SQL 行首括号。
        bf.push(b'(');
        for (i, receiver) in self.receivers.iter().enumerate() {
            receiver.WriteToBuffer(bf, escape_backslash);
            // 末列后不加分隔符。
            if i != self.receivers.len() - 1 {
                // 列间逗号分隔。
                bf.push(b',');
            }
        }
        // SQL 行尾括号。
        bf.push(b')');
    }

    fn GetRawBytes(&self) -> Vec<RawBytes> {
        let mut dst = Vec::with_capacity(self.receivers.len());
        self.appendRawBytes(&mut dst);
        dst
    }
}
impl crate::RowReceiverStringer for RowReceiverArr {}

/// 字符串/日期等：SQL 输出带单引号，NULL 写 `NULL` 字面量。
pub struct SQLTypeString {
    pub raw: RawBytes,
}
impl crate::RowReceiver for SQLTypeString {
    fn BindAddress(&mut self, arg: &mut [RawBytes]) {
        // RowReceiver 仅绑定首列 buffer。
        if let Some(a) = arg.get_mut(0) {
            // 克隆扫描缓冲区引用。
            self.raw = a.clone();
        }
    }
}
impl crate::Stringer for SQLTypeString {
    fn WriteToBuffer(&self, bf: &mut Vec<u8>, escape_backslash: bool) {
        if let Some(bytes) = self.raw.as_opt() {
            bf.push(b'\'');
            escapeSQL(bytes, bf, escape_backslash);
            bf.push(b'\'');
        } else {
            // SQL NULL 关键字。
            bf.extend_from_slice(NULL_VALUE.as_bytes());
        }
    }

    fn GetRawBytes(&self) -> Vec<RawBytes> {
        vec![self.raw.clone()]
    }
}
impl crate::RowReceiverStringer for SQLTypeString {}

/// 数值列：不加引号直接输出字节；NULL 仍写 `NULL`。
pub struct SQLTypeNumber {
    pub inner: SQLTypeString,
}
impl crate::RowReceiver for SQLTypeNumber {
    fn BindAddress(&mut self, arg: &mut [RawBytes]) {
        self.inner.BindAddress(arg);
    }
}
impl crate::Stringer for SQLTypeNumber {
    // 数值列忽略 backslash 开关。
    fn WriteToBuffer(&self, bf: &mut Vec<u8>, _escape_backslash: bool) {
        if let Some(bytes) = self.inner.raw.as_opt() {
            bf.extend_from_slice(bytes);
        } else {
            bf.extend_from_slice(NULL_VALUE.as_bytes());
        }
    }

    fn GetRawBytes(&self) -> Vec<RawBytes> {
        vec![self.inner.raw.clone()]
    }
}
impl crate::RowReceiverStringer for SQLTypeNumber {}

/// 二进制列：SQL 用 `x'hex'`。
pub struct SQLTypeBytes {
    pub raw: RawBytes,
}
impl crate::RowReceiver for SQLTypeBytes {
    fn BindAddress(&mut self, arg: &mut [RawBytes]) {
        if let Some(a) = arg.get_mut(0) {
            self.raw = a.clone();
        }
    }
}
impl crate::Stringer for SQLTypeBytes {
    fn WriteToBuffer(&self, bf: &mut Vec<u8>, _escape_backslash: bool) {
        if let Some(bytes) = self.raw.as_opt() {
            // 二进制 SQL 前缀 x'。
            bf.extend_from_slice(b"x'");
            // 小写 hex 无 0x 前缀。
            write_hex_bytes(bf, bytes);
            bf.push(b'\'');
        } else {
            bf.extend_from_slice(NULL_VALUE.as_bytes());
        }
    }

    fn GetRawBytes(&self) -> Vec<RawBytes> {
        vec![self.raw.clone()]
    }
}
impl crate::RowReceiverStringer for SQLTypeBytes {}

struct HexBytes<'a>(&'a [u8]);
impl std::fmt::LowerHex for HexBytes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            write!(f, "{:02x}", b)?;
        }
        Ok(())
    }
}

// Fix WriteToBuffer for SQLTypeBytes - can't write! into Vec<u8> with fmt::Write the same way for hex via write! macro targeting Vec
// Provide helper:
/// 小写十六进制写入 Vec，供 SQL `x'...'` 使用。
pub fn write_hex_bytes(bf: &mut Vec<u8>, bytes: &[u8]) {
    // 小写 hex 查表。
    const HEX: &[u8] = b"0123456789abcdef";
    for &b in bytes {
        // 高半字节。
        bf.push(HEX[(b >> 4) as usize]);
        // 低半字节。
        bf.push(HEX[(b & 0xf) as usize]);
    }
}

impl RowReceiverArr {
    pub fn appendRawBytes(&self, dst: &mut Vec<RawBytes>) {
        dst.extend(
            self.receivers
                .iter()
                .map(|r| r.GetRawBytes().into_iter().next().unwrap_or_default()),
        );
    }
}
