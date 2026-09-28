// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 比较与转换受语句 Context（截断/溢出标志）与校对规则影响。
// ENUM/SET/BIT、JSON、向量等；提供存取、比较、类型转换、序列化与内存估算。
// 用 Kind 标签区分 NULL、整数、浮点、字符串/字节、DECIMAL、时间、
//
// Datum：SQL 求值中的统一标量值容器（对齐 Go `types.Datum`）。
// 本文件由 pkg/types/datum.go 迁移而来，保留 Go 实现的存储、比较、转换和边界语义。
//

// Kind constants.
/// NULL。
// Kind 常量：标识 Datum 内实际存放的类型标签。
pub const KindNull: u8 = 0;
/// 有符号 64 位整数。
pub const KindInt64: u8 = 1;
/// 无符号 64 位整数。
pub const KindUint64: u8 = 2;
/// 32 位浮点。
pub const KindFloat32: u8 = 3;
/// 64 位浮点。
pub const KindFloat64: u8 = 4;
/// 字符串。
pub const KindString: u8 = 5;
/// 字节序列。
pub const KindBytes: u8 = 6;
/// BIT/HEX 字面量。
pub const KindBinaryLiteral: u8 = 7; // Used for BIT / HEX literals.
/// MySQL DECIMAL。
pub const KindMysqlDecimal: u8 = 8;
/// MySQL TIME/Duration。
pub const KindMysqlDuration: u8 = 9;
/// ENUM。
pub const KindMysqlEnum: u8 = 10;
/// 表列 BIT 值。
pub const KindMysqlBit: u8 = 11; // Used for BIT table column values.
/// SET。
pub const KindMysqlSet: u8 = 12;
/// 日期/日期时间。
pub const KindMysqlTime: u8 = 13;
/// 任意 interface。
pub const KindInterface: u8 = 14;
/// 最小非 NULL 哨兵。
pub const KindMinNotNull: u8 = 15;
/// 最大价值哨兵（+inf）。
pub const KindMaxValue: u8 = 16;
/// 原始字节。
pub const KindRaw: u8 = 17;
/// Binary JSON。
pub const KindMysqlJSON: u8 = 18;
/// float32 向量。
pub const KindVectorFloat32: u8 = 19;

// Datum is a data box holds different kind of data.
// Datum 是存放多种类型值的统一数据盒。
// Rust 用可共享的 Any 保存 Go interface{} 字段，使 Clone 保留其浅复制语义。
#[derive(Default)]
/// 统一标量盒：k/i/b/x 等字段按 Kind 解释。
pub struct Datum {
    k: u8,
    decimal: u16,
    length: u32,
    i: i64,
    collation: String,
    b: Vec<u8>,
    x: Option<std::sync::Arc<dyn any::Any + Send + Sync>>,
}

impl Clone for Datum {
    /// Clone：字节深拷贝，interface 浅复制（Arc）。
    fn clone(&self) -> Self {
        // Go 的 Clone 深拷贝 b、浅复制普通 interface，并在 Copy 中深拷贝 decimal/time。
        Datum {
            k: self.k,
            decimal: self.decimal,
            length: self.length,
            i: self.i,
            collation: self.collation.clone(),
            b: self.b.clone(),
            x: self.x.clone(),
        }
    }
}

// EmptyDatumSize is the size of empty datum.
// 空 Datum 的近似字节大小。
pub const EmptyDatumSize: i64 = 72;

impl Datum {
    // Clone create a deep copy of the Datum.
    // 创建 Datum 的深拷贝（返回 Box）。
    pub fn Clone(&self) -> Box<Datum> {
        let mut ret = Box::new(Datum::default());
        self.Copy(&mut ret);
        ret
    }

    // Copy deep copies a Datum into destination.
    // 将自身深拷贝写入目标 Datum；decimal/time 再拷贝载荷。
    pub fn Copy(&self, dst: &mut Datum) {
        *dst = self.clone();
        // decimal/time 在 Copy 中需真正深拷贝载荷
        match dst.Kind() {
            KindMysqlDecimal => {
                let d = self.GetMysqlDecimal().clone();
                dst.SetMysqlDecimal(d);
            }
            KindMysqlTime => {
                dst.SetMysqlTime(self.GetMysqlTime());
            }
            _ => {}
        }
    }

    // Kind gets the kind of the datum.
    // 返回类型标签 Kind。
    pub fn Kind(&self) -> u8 {
        self.k
    }

    // Collation gets the collation of the datum.
    // 返回字符串校对规则名。
    pub fn Collation(&self) -> String {
        self.collation.clone()
    }

    // SetCollation sets the collation of the datum.
    // 设置字符串校对规则名。
    pub fn SetCollation(&mut self, collation: String) {
        self.collation = collation;
    }

    // Frac gets the frac of the datum.
    // 返回小数位数（frac）。
    pub fn Frac(&self) -> i32 {
        self.decimal as i32
    }

    // SetFrac sets the frac of the datum.
    // 设置小数位数（frac）。
    pub fn SetFrac(&mut self, frac: i32) {
        self.decimal = frac as u16;
    }

    // Length gets the length of the datum.
    // 返回显示/存储长度。
    pub fn Length(&self) -> i32 {
        self.length as i32
    }

    // SetLength sets the length of the datum.
    // 设置显示/存储长度。
    pub fn SetLength(&mut self, l: i32) {
        self.length = l as u32;
    }

    // IsNull checks if datum is null.
    // 是否为 NULL（KindNull）。
    pub fn IsNull(&self) -> bool {
        self.k == KindNull
    }

    // GetInt64 gets int64 value.
    // 取有符号 64 位整数。
    pub fn GetInt64(&self) -> i64 {
        self.i
    }

    // SetInt64 sets int64 value.
    // 写入有符号 64 位整数并设 Kind。
    pub fn SetInt64(&mut self, i: i64) {
        self.k = KindInt64;
        self.i = i;
    }

    // GetUint64 gets uint64 value.
    // 取无符号 64 位整数。
    pub fn GetUint64(&self) -> u64 {
        self.i as u64
    }

    // SetUint64 sets uint64 value.
    // 写入无符号 64 位整数并设 Kind。
    pub fn SetUint64(&mut self, i: u64) {
        self.k = KindUint64;
        self.i = i as i64;
    }

    // GetFloat64 gets float64 value.
    // 取 float64（存于 i 的比特位）。
    pub fn GetFloat64(&self) -> f64 {
        f64::from_bits(self.i as u64)
    }

    // SetFloat64 sets float64 value.
    // 写入 float64 比特位并设 Kind。
    pub fn SetFloat64(&mut self, f: f64) {
        self.k = KindFloat64;
        self.i = f.to_bits() as i64;
    }

    // GetFloat32 gets float32 value.
    // 取 float32。
    pub fn GetFloat32(&self) -> f32 {
        f64::from_bits(self.i as u64) as f32
    }

    // SetFloat32 sets float32 value.
    // 写入 float32 并设 Kind。
    pub fn SetFloat32(&mut self, f: f32) {
        self.k = KindFloat32;
        self.i = (f as f64).to_bits() as i64;
    }

    // SetFloat32FromF64 sets float32 values from f64.
    // 用 f64 比特写入 KindFloat32（对齐 Go）。
    pub fn SetFloat32FromF64(&mut self, f: f64) {
        self.k = KindFloat32;
        self.i = f.to_bits() as i64;
    }

    // GetString gets string value.
    // 按字节缓冲解释为字符串。
    pub fn GetString(&self) -> String {
        hack::String(&self.b).to_string()
    }

    // GetBinaryStringEncoded gets the string value encoded with given charset.
    // 按校对对应字符集编码后返回字符串。
    pub fn GetBinaryStringEncoded(&self) -> String {
        let coll = match charset::GetCollationByName(&self.Collation()) {
            Ok(coll) => coll,
            Err(_) => {
                return self.GetString();
            }
        };
        let enc = charset::FindEncodingTakeUTF8AsNoop(&coll.CharsetName);
        let mut output = Vec::new();
        match enc.Transform(&mut output, &self.b, charset::OpEncodeNoErr) {
            Ok(replace) => hack::String(&replace).to_string(),
            Err(_) => self.GetString(),
        }
    }

    // GetBinaryStringDecoded gets the string value decoded with given charset.
    // 按字符集解码字节为字符串。
    pub fn GetBinaryStringDecoded(&self, flags: Flags, chs: &str) -> Result<String, errors::Error> {
        let (enc, skip) = findEncoding(flags, chs);
        if skip {
            return Ok(self.GetString());
        }
        let mut output = Vec::new();
        let trim = enc
            .Transform(&mut output, &self.b, charset::OpDecode)
            .map_err(|err| errors::New(err.to_string()))?;
        Ok(hack::String(&trim).to_string())
    }

    // GetStringWithCheck gets the string and checks if it is valid in a given charset.
    // 取字符串并校验是否合法于指定字符集。
    pub fn GetStringWithCheck(&self, flags: Flags, chs: &str) -> Result<String, errors::Error> {
        let (enc, skip) = findEncoding(flags, chs);
        if skip {
            return Ok(self.GetString());
        }
        let str_value = self.GetBytes();
        if !enc.IsValid(&str_value) {
            let mut output = Vec::new();
            let replace = enc
                .Transform(&mut output, &str_value, charset::OpReplace)
                .map_err(|err| errors::New(err.to_string()))?;
            return Ok(hack::String(&replace).to_string());
        }
        Ok(self.GetString())
    }

    // SetString sets string value.
    // 写入字符串与校对规则。
    pub fn SetString(&mut self, s: String, collation: String) {
        self.k = KindString;
        sink(&s);
        self.b = hack::Slice(&s);
        self.collation = collation;
    }

    // GetBytes gets bytes value.
    // 取原始字节。
    pub fn GetBytes(&self) -> Vec<u8> {
        self.b.clone()
    }

    // SetBytes sets bytes value to datum.
    // 写入字节并设 KindBytes。
    pub fn SetBytes(&mut self, b: Vec<u8>) {
        self.k = KindBytes;
        self.b = b;
        self.collation = charset::CollationBin.to_string();
    }

    // SetBytesAsString sets bytes value to datum as string type.
    // 以字符串 Kind 写入字节并记录长度。
    pub fn SetBytesAsString(&mut self, b: Vec<u8>, collation: String, length: u32) {
        self.k = KindString;
        self.b = b;
        self.length = length;
        self.collation = collation;
    }

    // GetInterface gets interface value.
    // 取 interface{} 对应的 Any。
    pub fn GetInterface(&self) -> Option<&std::sync::Arc<dyn any::Any + Send + Sync>> {
        self.x.as_ref()
    }

    // SetInterface sets interface to datum.
    // 写入任意 interface 值。
    pub fn SetInterface(&mut self, x: Box<dyn any::Any + Send + Sync>) {
        self.k = KindInterface;
        self.x = Some(x.into());
    }

    // SetNull sets datum to nil.
    // 置为 NULL。
    pub fn SetNull(&mut self) {
        self.k = KindNull;
        self.x = None;
    }

    // SetMinNotNull sets datum to minNotNull value.
    // 置为 MinNotNull 哨兵。
    pub fn SetMinNotNull(&mut self) {
        self.k = KindMinNotNull;
        self.x = None;
    }

    // GetBinaryLiteral4Cmp gets Bit value, and remove it's prefix 0 for comparison.
    // 取二进制字面量并去掉前导零，供比较。
    pub fn GetBinaryLiteral4Cmp(&self) -> BinaryLiteral {
        let bitLen = self.b.len();
        if bitLen == 0 {
            return BinaryLiteral(Vec::new());
        }
        for i in 0..bitLen {
            if self.b[i] != 0 {
                return BinaryLiteral(self.b[i..].to_vec());
            }
        }
        BinaryLiteral(self.b[bitLen - 1..].to_vec())
    }

    // GetBinaryLiteral gets Bit value.
    // 取 BIT/HEX 二进制字面量。
    pub fn GetBinaryLiteral(&self) -> BinaryLiteral {
        BinaryLiteral(self.b.clone())
    }

    // GetMysqlBit gets MysqlBit value.
    // 取表列 BIT 值。
    pub fn GetMysqlBit(&self) -> BinaryLiteral {
        self.GetBinaryLiteral()
    }

    // SetBinaryLiteral sets Bit value.
    // 写入二进制字面量。
    pub fn SetBinaryLiteral(&mut self, b: BinaryLiteral) {
        self.k = KindBinaryLiteral;
        self.b = b.0;
        self.collation = charset::CollationBin.to_string();
    }

    // SetMysqlBit sets MysqlBit value.
    // 写入表列 BIT。
    pub fn SetMysqlBit(&mut self, b: BinaryLiteral) {
        self.k = KindMysqlBit;
        self.b = b.0;
    }

    // GetMysqlDecimal gets decimal value.
    // 取 MyDecimal。
    pub fn GetMysqlDecimal(&self) -> MyDecimal {
        self.x
            .as_ref()
            .and_then(|x| x.downcast_ref::<MyDecimal>())
            .cloned()
            .unwrap_or_default()
    }

    // SetMysqlDecimal sets decimal value.
    // 写入 MyDecimal。
    pub fn SetMysqlDecimal(&mut self, b: MyDecimal) {
        self.k = KindMysqlDecimal;
        self.x = Some(std::sync::Arc::new(b));
    }

    // GetMysqlDuration gets Duration value.
    // 取 Duration。
    pub fn GetMysqlDuration(&self) -> Duration {
        Duration {
            Duration: self.i,
            Fsp: self.decimal as i8 as i32,
        }
    }

    // SetMysqlDuration sets Duration value.
    // 写入 Duration。
    pub fn SetMysqlDuration(&mut self, b: Duration) {
        self.k = KindMysqlDuration;
        self.i = b.Duration;
        self.decimal = b.Fsp as u16;
    }

    // GetMysqlEnum gets Enum value.
    // 取 ENUM。
    pub fn GetMysqlEnum(&self) -> Enum {
        Enum {
            Value: self.i as u64,
            Name: hack::String(&self.b).to_string(),
        }
    }

    // SetMysqlEnum sets Enum value.
    // 写入 ENUM。
    pub fn SetMysqlEnum(&mut self, b: Enum, collation: String) {
        self.k = KindMysqlEnum;
        self.i = b.Value as i64;
        sink(&b.Name);
        self.collation = collation;
        self.b = hack::Slice(&b.Name);
    }

    // GetMysqlSet gets Set value.
    // 取 SET。
    pub fn GetMysqlSet(&self) -> Set {
        Set {
            Value: self.i as u64,
            Name: hack::String(&self.b).to_string(),
        }
    }

    // SetMysqlSet sets Set value.
    // 写入 SET。
    pub fn SetMysqlSet(&mut self, b: Set, collation: String) {
        self.k = KindMysqlSet;
        self.i = b.Value as i64;
        sink(&b.Name);
        self.collation = collation;
        self.b = hack::Slice(&b.Name);
    }

    // GetMysqlJSON gets json.BinaryJSON value.
    // 取 BinaryJSON。
    pub fn GetMysqlJSON(&self) -> BinaryJSON {
        BinaryJSON {
            TypeCode: self.i as u8,
            Value: self.b.clone(),
        }
    }

    // SetMysqlJSON sets json.BinaryJSON value.
    // 写入 BinaryJSON。
    pub fn SetMysqlJSON(&mut self, b: BinaryJSON) {
        self.k = KindMysqlJSON;
        self.i = b.TypeCode as i64;
        self.b = b.Value;
    }

    // SetVectorFloat32 sets VectorFloat32 value.
    // 写入向量 float32。
    pub fn SetVectorFloat32(&mut self, vec: VectorFloat32) {
        self.k = KindVectorFloat32;
        self.b = vec.ZeroCopySerialize().to_vec();
    }

    // GetVectorFloat32 gets VectorFloat32 value.
    // 取向量 float32。
    pub fn GetVectorFloat32(&self) -> VectorFloat32 {
        ZeroCopyDeserializeVectorFloat32(&self.b)
            .map(|(value, _)| value)
            .unwrap_or_else(|err| panic!("{err}"))
    }

    // GetMysqlTime gets types.Time value.
    // 取 MySQL Time。
    pub fn GetMysqlTime(&self) -> Time {
        self.x
            .as_ref()
            .and_then(|x| x.downcast_ref::<Time>())
            .cloned()
            .unwrap_or_default()
    }

    // SetMysqlTime sets types.Time value.
    // 写入 MySQL Time。
    pub fn SetMysqlTime(&mut self, b: Time) {
        self.k = KindMysqlTime;
        self.x = Some(std::sync::Arc::new(b));
    }

    // SetRaw sets raw value.
    // 写入原始字节（KindRaw）。
    pub fn SetRaw(&mut self, b: Vec<u8>) {
        self.k = KindRaw;
        self.b = b;
    }

    // GetRaw gets raw value.
    // 取原始字节。
    pub fn GetRaw(&self) -> Vec<u8> {
        self.b.clone()
    }

    // SetAutoID set the auto increment ID according to its int flag.
    // 按无符号标志写入自增 ID。
    pub fn SetAutoID(&mut self, id: i64, flag: u32) {
        if mysql::HasUnsignedFlag(flag as usize) {
            self.SetUint64(id as u64);
        } else {
            self.SetInt64(id);
        }
    }

    // String returns a human-readable description of Datum. It is intended only for debugging.
    // 调试用可读描述。
    pub fn String(&self) -> String {
        let t = match self.k {
            KindNull => "KindNull",
            KindInt64 => "KindInt64",
            KindUint64 => "KindUint64",
            KindFloat32 => "KindFloat32",
            KindFloat64 => "KindFloat64",
            KindString => "KindString",
            KindBytes => "KindBytes",
            KindBinaryLiteral => "KindBinaryLiteral",
            KindMysqlDecimal => "KindMysqlDecimal",
            KindMysqlDuration => "KindMysqlDuration",
            KindMysqlEnum => "KindMysqlEnum",
            KindMysqlBit => "KindMysqlBit",
            KindMysqlSet => "KindMysqlSet",
            KindMysqlTime => "KindMysqlTime",
            KindInterface => "KindInterface",
            KindMinNotNull => "KindMinNotNull",
            KindMaxValue => "KindMaxValue",
            KindRaw => "KindRaw",
            KindMysqlJSON => "KindMysqlJSON",
            KindVectorFloat32 => "KindVectorFloat32",
            _ => "Unknown",
        };
        let mut v = self.GetValueStringForDebug();
        if self.k == KindBytes || self.k == KindString {
            // Go 使用 %q 后去掉两端引号，只保留转义效果。
            v = strconv::Quote(&v).trim_matches('"').to_string();
        }
        format!("{} {}", t, v)
    }

    // GetValue gets the value of the datum of any kind.
    // 按 Kind 取出对应动态值。
    pub fn GetValue(&self) -> DatumValue {
        match self.k {
            KindInt64 => DatumValue::Int64(self.GetInt64()),
            KindUint64 => DatumValue::Uint64(self.GetUint64()),
            KindFloat32 => DatumValue::Float32(self.GetFloat32()),
            KindFloat64 => DatumValue::Float64(self.GetFloat64()),
            KindString => DatumValue::String(self.GetString()),
            KindBytes => DatumValue::Bytes(self.GetBytes()),
            KindMysqlDecimal => DatumValue::Decimal(self.GetMysqlDecimal()),
            KindMysqlDuration => DatumValue::Duration(self.GetMysqlDuration()),
            KindMysqlEnum => DatumValue::Enum(self.GetMysqlEnum()),
            KindBinaryLiteral | KindMysqlBit => DatumValue::BinaryLiteral(self.GetBinaryLiteral()),
            KindMysqlSet => DatumValue::Set(self.GetMysqlSet()),
            KindMysqlJSON => DatumValue::JSON(self.GetMysqlJSON()),
            KindMysqlTime => DatumValue::Time(self.GetMysqlTime()),
            KindVectorFloat32 => DatumValue::VectorFloat32(self.GetVectorFloat32()),
            _ => DatumValue::Interface,
        }
    }

    // GetValueStringForDebug 对应 Go String 中 fmt.Sprintf("%v", d.GetValue())。
    fn GetValueStringForDebug(&self) -> String {
        match self.GetValue() {
            DatumValue::Int64(value) => value.to_string(),
            DatumValue::Uint64(value) => value.to_string(),
            DatumValue::String(s) => s,
            DatumValue::Bytes(b) => hack::String(&b).to_string(),
            // Go 的 %v 直接打印浮点；Rust Debug 会带 Float64(...) 包装，故手动格式化。
            // Go's `%v` formats primitive floating values directly; deriving
            // Debug for the Rust enum would leak the `Float64(...)` wrapper.
            DatumValue::Float32(value) => {
                strconv::FormatFloat(value as f64, b'f', -1, 32).to_string()
            }
            DatumValue::Float64(value) => strconv::FormatFloat(value, b'f', -1, 64).to_string(),
            DatumValue::Decimal(value) => value.String(),
            DatumValue::Interface if self.GetInterface().is_none() => "<nil>".to_string(),
            v => format!("{:?}", v),
        }
    }

    // TruncatedStringify returns the %v representation of the datum but truncated.
    // 截断后的调试字符串表示。
    pub fn TruncatedStringify(&self) -> String {
        match self.k {
            KindString | KindBytes => truncateStringIfNeeded(self.GetString()),
            KindMysqlJSON => truncateStringIfNeeded(self.GetMysqlJSON().String()),
            KindVectorFloat32 => self.GetVectorFloat32().TruncatedString(),
            KindMysqlTime => self.GetMysqlTime().String(),
            KindMysqlDuration => self.GetMysqlDuration().String(),
            KindMysqlDecimal => self.GetMysqlDecimal().String(),
            KindMysqlEnum => self.GetMysqlEnum().String(),
            KindMysqlSet => self.GetMysqlSet().String(),
            KindInt64 => self.GetInt64().to_string(),
            KindUint64 => self.GetUint64().to_string(),
            _ => self.GetValueStringForDebug(),
        }
    }

    // SetValueWithDefaultCollation sets any kind of value.
    // 按值类型写入，使用默认校对规则。
    pub fn SetValueWithDefaultCollation(&mut self, val: &dyn any::Any) {
        // Go 的 type switch 在 Rust 中用 downcast_ref 表达，默认 collation 使用 mysql.DefaultCollationName。
        if val.is::<()>() {
            self.SetNull();
        } else if let Some(x) = val.downcast_ref::<bool>() {
            self.SetInt64(if *x { 1 } else { 0 });
        } else if let Some(x) = val.downcast_ref::<i32>() {
            self.SetInt64(*x as i64);
        } else if let Some(x) = val.downcast_ref::<i64>() {
            self.SetInt64(*x);
        } else if let Some(x) = val.downcast_ref::<u64>() {
            self.SetUint64(*x);
        } else if let Some(x) = val.downcast_ref::<f32>() {
            self.SetFloat32(*x);
        } else if let Some(x) = val.downcast_ref::<f64>() {
            self.SetFloat64(*x);
        } else if let Some(x) = val.downcast_ref::<String>() {
            self.SetString(x.clone(), mysql::DefaultCollationName.to_string());
        } else if let Some(x) = val.downcast_ref::<Vec<u8>>() {
            self.SetBytes(x.clone());
        } else if let Some(x) = val.downcast_ref::<MyDecimal>() {
            self.SetMysqlDecimal(x.clone());
        } else if let Some(x) = val.downcast_ref::<Duration>() {
            self.SetMysqlDuration(*x);
        } else if let Some(x) = val.downcast_ref::<Enum>() {
            self.SetMysqlEnum(x.clone(), mysql::DefaultCollationName.to_string());
        } else if let Some(x) = val.downcast_ref::<BinaryLiteral>() {
            self.SetBinaryLiteral(x.clone());
        } else if let Some(x) = val.downcast_ref::<Set>() {
            self.SetMysqlSet(x.clone(), mysql::DefaultCollationName.to_string());
        } else if let Some(x) = val.downcast_ref::<BinaryJSON>() {
            self.SetMysqlJSON(x.clone());
        } else if let Some(x) = val.downcast_ref::<Time>() {
            self.SetMysqlTime(x.clone());
        } else if let Some(x) = val.downcast_ref::<VectorFloat32>() {
            self.SetVectorFloat32(x.Clone());
        }
    }

    // SetValue sets any kind of value.
    // 按值类型与指定校对规则写入。
    pub fn SetValue(&mut self, val: &dyn any::Any, tp: &types::FieldType) {
        if let Some(x) = val.downcast_ref::<String>() {
            self.SetString(x.clone(), tp.GetCollate().to_owned());
        } else if let Some(x) = val.downcast_ref::<Enum>() {
            self.SetMysqlEnum(x.clone(), tp.GetCollate().to_owned());
        } else if let Some(x) = val.downcast_ref::<Set>() {
            self.SetMysqlSet(x.clone(), tp.GetCollate().to_owned());
        } else {
            self.SetValueWithDefaultCollation(val);
        }
    }

    // Hash64 implements base.HashEquals<0th> interface.
    // 写入哈希器（HashEquals）。
    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        unsafe { Hash64ForDatum(h, self) };
    }

    // Equals implements base.HashEquals.<1st> interface.
    // 语义相等比较（HashEquals）。
    pub fn Equals(&self, other: &dyn any::Any) -> bool {
        let Some(d2) = other.downcast_ref::<Datum>() else {
            return false;
        };
        let ok = self.k == d2.k
            && self.decimal == d2.decimal
            && self.length == d2.length
            && self.i == d2.i
            && self.collation == d2.collation
            && self.b == d2.b;
        if !ok {
            return false;
        }
        match self.k {
            KindMysqlDecimal => self.GetMysqlDecimal().Compare(&d2.GetMysqlDecimal()) == 0,
            KindMysqlTime => self.GetMysqlTime().Compare(d2.GetMysqlTime()) == 0,
            _ => true,
        }
    }

    // Compare compares datum to another datum.
    // 按 Kind 矩阵分派比较；NULL/MinNotNull/MaxValue 有特殊序
    // 在 Context 与校对器下比较两个 Datum。
    pub fn Compare(
        &self,
        ctx: Context,
        ad: &Datum,
        comparer: &dyn collate::Collator,
    ) -> Result<i32, errors::Error> {
        if self.k == KindMysqlJSON && ad.k != KindMysqlJSON {
            let cmp = ad.Compare(ctx, self, comparer)?;
            return Ok(cmp * -1);
        }
        match ad.k {
            KindNull => Ok(if self.k == KindNull { 0 } else { 1 }),
            KindMinNotNull => {
                if self.k == KindNull {
                    Ok(-1)
                } else if self.k == KindMinNotNull {
                    Ok(0)
                } else {
                    Ok(1)
                }
            }
            KindMaxValue => Ok(if self.k == KindMaxValue { 0 } else { -1 }),
            KindInt64 => self.compareInt64(ctx, ad.GetInt64()),
            KindUint64 => self.compareUint64(ctx, ad.GetUint64()),
            KindFloat32 | KindFloat64 => self.compareFloat64(ctx, ad.GetFloat64()),
            KindString | KindBytes => self.compareStringBytes(ctx, &ad.GetBytes(), comparer),
            KindMysqlDecimal => self.compareMysqlDecimal(ctx, &ad.GetMysqlDecimal()),
            KindMysqlDuration => self.compareMysqlDuration(ctx, ad.GetMysqlDuration()),
            KindMysqlEnum => self.compareMysqlEnum(ctx, ad.GetMysqlEnum(), comparer),
            KindBinaryLiteral | KindMysqlBit => {
                self.compareBinaryLiteral(ctx, ad.GetBinaryLiteral4Cmp(), comparer)
            }
            KindMysqlSet => self.compareMysqlSet(ctx, ad.GetMysqlSet(), comparer),
            KindMysqlJSON => self.compareMysqlJSON(ad.GetMysqlJSON()),
            KindMysqlTime => self.compareMysqlTime(ctx, ad.GetMysqlTime()),
            KindVectorFloat32 => self.compareVectorFloat32(ctx, ad.GetVectorFloat32()),
            _ => Ok(0),
        }
    }

    // compareInt64 对应 Go 的 int64 目标比较，uint64 超出 MaxInt64 时单独处理。
    pub fn compareInt64(&self, ctx: Context, i: i64) -> Result<i32, errors::Error> {
        match self.k {
            KindMaxValue => Ok(1),
            KindInt64 => Ok(cmp::Compare(self.i, i)),
            KindUint64 => {
                if i < 0 || self.GetUint64() > math::MaxInt64 as u64 {
                    return Ok(1);
                }
                Ok(cmp::Compare(self.i, i))
            }
            _ => self.compareFloat64(ctx, i as f64),
        }
    }

    // compareUint64 对应 Go 的 uint64 目标比较。
    pub fn compareUint64(&self, ctx: Context, u: u64) -> Result<i32, errors::Error> {
        match self.k {
            KindMaxValue => Ok(1),
            KindInt64 => {
                if self.i < 0 || u > math::MaxInt64 as u64 {
                    return Ok(-1);
                }
                Ok(cmp::Compare(self.i, u as i64))
            }
            KindUint64 => Ok(cmp::Compare(self.GetUint64(), u)),
            _ => self.compareFloat64(ctx, u as f64),
        }
    }

    // compareFloat64 对应 Go 的跨类型浮点比较，大多数非数值类型先转成 float。
    pub fn compareFloat64(&self, ctx: Context, f: f64) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindInt64 => Ok(cmp::Compare(self.i as f64, f)),
            KindUint64 => Ok(cmp::Compare(self.GetUint64() as f64, f)),
            KindFloat32 | KindFloat64 => Ok(cmp::Compare(self.GetFloat64(), f)),
            KindString | KindBytes => {
                let fVal = StrToFloat(ctx, &self.GetString(), false)?;
                Ok(cmp::Compare(fVal, f))
            }
            KindMysqlDecimal => {
                let fVal = self.GetMysqlDecimal().ToFloat64()?;
                Ok(cmp::Compare(fVal, f))
            }
            KindMysqlDuration => Ok(cmp::Compare(duration_seconds(self.GetMysqlDuration()), f)),
            KindMysqlEnum => Ok(cmp::Compare(self.GetMysqlEnum().ToNumber(), f)),
            KindBinaryLiteral | KindMysqlBit => {
                let val = self.GetBinaryLiteral4Cmp().ToInt(ctx)?;
                Ok(cmp::Compare(val as f64, f))
            }
            KindMysqlSet => Ok(cmp::Compare(self.GetMysqlSet().ToNumber(), f)),
            KindMysqlTime => {
                let fVal = rust_decimal_to_f64(self.GetMysqlTime().ToNumber());
                Ok(cmp::Compare(fVal, f))
            }
            _ => Ok(-1),
        }
    }

    // compareString 对应 Go 的字符串目标比较，collator 由调用者传入。
    pub fn compareString(
        &self,
        ctx: Context,
        s: &str,
        comparer: &dyn collate::Collator,
    ) -> Result<i32, errors::Error> {
        self.compareStringBytes(ctx, s.as_bytes(), comparer)
    }

    /// compareStringBytes preserves Go string bytes until an operation
    /// explicitly requires decoded text.
    pub fn compareStringBytes(
        &self,
        ctx: Context,
        s: &[u8],
        comparer: &dyn collate::Collator,
    ) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindString | KindBytes => Ok(comparer.CompareBytes(&self.GetBytes(), s)),
            KindMysqlDecimal => {
                let mut dec = MyDecimal::default();
                if let Err(error) = dec.FromString(s) {
                    ctx.HandleTruncate((), contextutil::errors::New(error.to_string()))
                        .map_err(errors::Error::from)?;
                }
                Ok(self.GetMysqlDecimal().Compare(&dec) as i32)
            }
            KindMysqlTime => {
                let s = String::from_utf8_lossy(s);
                let dt = ParseDatetime(&time_context(&ctx), &s)?;
                Ok(self.GetMysqlTime().Compare(dt))
            }
            KindMysqlDuration => {
                let s = String::from_utf8_lossy(s);
                let (dur, _) = ParseDuration(&time_context(&ctx), &s, MaxFsp)?;
                Ok(self.GetMysqlDuration().Compare(dur))
            }
            KindMysqlSet => Ok(comparer.CompareBytes(self.GetMysqlSet().String().as_bytes(), s)),
            KindMysqlEnum => Ok(comparer.CompareBytes(self.GetMysqlEnum().String().as_bytes(), s)),
            KindBinaryLiteral | KindMysqlBit => {
                Ok(comparer.CompareBytes(self.GetBinaryLiteral4Cmp().ToString().as_bytes(), s))
            }
            _ => {
                let s = String::from_utf8_lossy(s);
                let fVal = StrToFloat(ctx.clone(), &s, false)?;
                self.compareFloat64(ctx, fVal)
            }
        }
    }

    // compareMysqlDecimal 对应 Go 的 decimal 目标比较。
    pub fn compareMysqlDecimal(&self, ctx: Context, dec: &MyDecimal) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindMysqlDecimal => Ok(self.GetMysqlDecimal().Compare(dec) as i32),
            KindString | KindBytes => {
                let mut dDec = MyDecimal::default();
                let bytes = self.GetBytes();
                if let Err(error) = dDec.FromString(&bytes) {
                    ctx.HandleTruncate((), contextutil::errors::New(error.to_string()))
                        .map_err(errors::Error::from)?;
                }
                Ok(dDec.Compare(dec) as i32)
            }
            _ => {
                let dVal = self.ConvertTo(ctx, &NewFieldType(mysql::TypeNewDecimal))?;
                Ok(dVal.GetMysqlDecimal().Compare(dec) as i32)
            }
        }
    }

    // compareMysqlDuration 对应 Go 的 duration 目标比较。
    pub fn compareMysqlDuration(&self, ctx: Context, dur: Duration) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindMysqlDuration => Ok(self.GetMysqlDuration().Compare(dur)),
            KindString | KindBytes => {
                let (dDur, _) = ParseDuration(&time_context(&ctx), &self.GetString(), MaxFsp)?;
                Ok(dDur.Compare(dur))
            }
            _ => self.compareFloat64(ctx, duration_seconds(dur)),
        }
    }

    // compareMysqlEnum 对应 Go 的 enum 目标比较。
    pub fn compareMysqlEnum(
        &self,
        sc: Context,
        enum_value: Enum,
        comparer: &dyn collate::Collator,
    ) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindString | KindBytes | KindMysqlEnum | KindMysqlSet => {
                Ok(comparer.Compare(&self.GetString(), &enum_value.String()))
            }
            _ => self.compareFloat64(sc, enum_value.ToNumber()),
        }
    }

    // compareBinaryLiteral 对应 Go 的 BIT/HEX literal 目标比较。
    pub fn compareBinaryLiteral(
        &self,
        ctx: Context,
        b: BinaryLiteral,
        comparer: &dyn collate::Collator,
    ) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindString | KindBytes | KindBinaryLiteral | KindMysqlBit => {
                Ok(comparer.Compare(&self.GetBinaryLiteral4Cmp().ToString(), &b.ToString()))
            }
            _ => {
                let val = b.ToInt(ctx.clone())?;
                self.compareFloat64(ctx, val as f64)
            }
        }
    }

    // compareMysqlSet 对应 Go 的 set 目标比较。
    pub fn compareMysqlSet(
        &self,
        ctx: Context,
        set: Set,
        comparer: &dyn collate::Collator,
    ) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindString | KindBytes | KindMysqlEnum | KindMysqlSet => {
                Ok(comparer.Compare(&self.GetString(), &set.String()))
            }
            _ => self.compareFloat64(ctx, set.ToNumber()),
        }
    }

    // compareMysqlJSON 对应 Go 的 JSON 比较；JSON 不等于 NULL。
    pub fn compareMysqlJSON(&self, target: BinaryJSON) -> Result<i32, errors::Error> {
        if self.k == KindNull {
            return Ok(1);
        }
        let origin = self.ToMysqlJSON()?;
        Ok(CompareBinaryJSON(&origin, &target))
    }

    // compareMysqlTime 对应 Go 的 Time 目标比较。
    pub fn compareMysqlTime(&self, ctx: Context, time_value: Time) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindString | KindBytes => {
                let dt = ParseDatetime(&time_context(&ctx), &self.GetString())?;
                Ok(dt.Compare(time_value))
            }
            KindMysqlTime => Ok(self.GetMysqlTime().Compare(time_value)),
            _ => {
                let fVal = rust_decimal_to_f64(time_value.ToNumber());
                self.compareFloat64(ctx, fVal)
            }
        }
    }

    // compareVectorFloat32 对应 Go 的 vector 目标比较。
    pub fn compareVectorFloat32(
        &self,
        _ctx: Context,
        vec: VectorFloat32,
    ) -> Result<i32, errors::Error> {
        match self.k {
            KindNull | KindMinNotNull => Ok(-1),
            KindMaxValue => Ok(1),
            KindVectorFloat32 => Ok(self.GetVectorFloat32().Compare(&vec)),
            _ => Err(errors::New(
                "cannot compare vector and non-vector, cast is required",
            )),
        }
    }
}

// DatumValue 对应 Go GetValue 的 interface{} 返回；这里用于保留不同 kind 的动态值形状。
#[derive(Debug)]
/// 从 Datum 抽出的动态值枚举。
pub enum DatumValue {
    Int64(i64),
    Uint64(u64),
    Float32(f32),
    Float64(f64),
    String(String),
    Bytes(Vec<u8>),
    Decimal(MyDecimal),
    Duration(Duration),
    Enum(Enum),
    BinaryLiteral(BinaryLiteral),
    Set(Set),
    JSON(BinaryJSON),
    Time(Time),
    VectorFloat32(VectorFloat32),
    Interface,
}

// sink prevents s from being allocated on the stack.
// 调试用空操作，吞掉字符串。
pub fn sink(_s: &str) {}

// findEncoding 对应 Go 的 charset 查找和 flags 跳过逻辑。
pub fn findEncoding(flags: Flags, chs: &str) -> (charset::EncodingRef, bool) {
    let mut enc = charset::FindEncoding(chs);
    if (enc.Tp() == charset::EncodingTpUTF8 && flags.SkipUTF8Check())
        || (enc.Tp() == charset::EncodingTpASCII && flags.SkipASCIICheck())
    {
        return (enc, true);
    }
    if chs == charset::CharsetUTF8 && !flags.SkipUTF8MB4Check() {
        enc = charset::EncodingUTF8MB3StrictImpl();
    }
    (enc, false)
}

// truncateStringIfNeeded 对应 Go 的 64 字节调试输出截断。
pub fn truncateStringIfNeeded(str_value: String) -> String {
    // 调试字符串最大长度上限
    const maxLen: usize = 64;
    if str_value.len() > maxLen {
        let suffix = "...(len:";
        return format!("{}{}{})", &str_value[..maxLen], suffix, str_value.len());
    }
    str_value
}

// 由 codec 包注入的 Hash64 实现，避免循环依赖。
// Hash64ForDatum is a hash function initialized by codec package.
pub static mut Hash64ForDatum: fn(&mut dyn base::Hasher, &Datum) = |_h, _d| {};

// TraceCmp 对应 compare helpers 中 `return cmp, errors.Trace(err)` 的形状。
fn TraceCmp(cmp: i32, err: Option<errors::Error>) -> Result<i32, errors::Error> {
    match err {
        Some(err) => Err(errors::Trace(err)),
        None => Ok(cmp),
    }
}

/// 由 Time 构造 BinaryJSON。
fn CreateBinaryJSONFromTime(value: Time) -> BinaryJSON {
    let type_code = match value.Type() {
        mysql::TypeDate => JSONTypeCodeDate,
        mysql::TypeTimestamp => JSONTypeCodeTimestamp,
        _ => JSONTypeCodeDatetime,
    };
    CreateBinaryJSON(JsonTime {
        CoreTime: value.coreTime.0,
        TypeCode: type_code,
        Fsp: value.Fsp() as u8,
    })
}

/// 由 Duration 构造 BinaryJSON。
fn CreateBinaryJSONFromDuration(value: Duration) -> BinaryJSON {
    CreateBinaryJSON(JsonDuration {
        Duration: value.Duration,
        Fsp: value.Fsp as u32,
    })
}

impl Datum {
    // ConvertTo converts a datum to the target field type.
    // 按目标 tp 分派 convertTo*；失败走 invalidConv
    // 按目标 FieldType 做类型转换。
    pub fn ConvertTo(&self, ctx: Context, target: &FieldType) -> Result<Datum, errors::Error> {
        if self.k == KindNull {
            return Ok(Datum::default());
        }
        match target.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong => {
                if mysql::HasUnsignedFlag(target.GetFlag()) {
                    self.convertToUint(ctx, target)
                } else {
                    self.convertToInt(ctx, target)
                }
            }
            mysql::TypeFloat | mysql::TypeDouble => self.convertToFloat(ctx, target),
            mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob
            | mysql::TypeString
            | mysql::TypeVarchar
            | mysql::TypeVarString => self.convertToString(ctx, target),
            mysql::TypeTimestamp => self.convertToMysqlTimestamp(ctx, target),
            mysql::TypeDatetime | mysql::TypeDate => self.convertToMysqlTime(ctx, target),
            mysql::TypeDuration => self.convertToMysqlDuration(ctx, target),
            mysql::TypeNewDecimal => self.convertToMysqlDecimal(ctx, target),
            mysql::TypeYear => self.ConvertToMysqlYear(ctx, target),
            mysql::TypeEnum => self.convertToMysqlEnum(ctx, target),
            mysql::TypeBit => self.convertToMysqlBit(ctx, target),
            mysql::TypeSet => self.convertToMysqlSet(ctx, target),
            mysql::TypeJSON => self.convertToMysqlJSON(target),
            mysql::TypeTiDBVectorFloat32 => self.convertToVectorFloat32(ctx, target),
            mysql::TypeNull => Ok(Datum::default()),
            _ => panic!("should never happen"),
        }
    }

    // convertToFloat 对应 Go 的 Datum 转 float/double。
    pub fn convertToFloat(&self, ctx: Context, target: &FieldType) -> Result<Datum, errors::Error> {
        let mut err: Option<errors::Error> = None;
        let f = match self.k {
            KindNull => return Ok(Datum::default()),
            KindInt64 => self.GetInt64() as f64,
            KindUint64 => self.GetUint64() as f64,
            KindFloat32 | KindFloat64 => self.GetFloat64(),
            KindString | KindBytes => match StrToFloat(ctx.clone(), &self.GetString(), false) {
                Ok(v) => v,
                Err(e) => {
                    err = Some(e.into());
                    0.0
                }
            },
            KindMysqlTime => rust_decimal_to_f64(self.GetMysqlTime().ToNumber()),
            KindMysqlDuration => rust_decimal_to_f64(self.GetMysqlDuration().ToNumber()),
            KindMysqlDecimal => self.GetMysqlDecimal().ToFloat64()?,
            KindMysqlSet => self.GetMysqlSet().ToNumber(),
            KindMysqlEnum => self.GetMysqlEnum().ToNumber(),
            KindBinaryLiteral | KindMysqlBit => self.GetBinaryLiteral().ToInt(ctx.clone())? as f64,
            KindMysqlJSON => ConvertJSONToFloat(ctx.clone(), self.GetMysqlJSON())?,
            _ => return invalidConv(self, target.GetType()),
        };
        let (f, err1) = ProduceFloatWithSpecifiedTp(f, target);
        if err.is_none() {
            err = err1;
        }
        let mut ret = Datum::default();
        if target.GetType() == mysql::TypeFloat {
            ret.SetFloat32(f as f32);
        } else {
            ret.SetFloat64(f);
        }
        match err {
            Some(err) => Err(errors::Trace(err)),
            None => Ok(ret),
        }
    }

    // convertToString 对应 Go 的 Datum 转 string/blob，保留字符集编码、解码、校验和长度生产规则。
    pub fn convertToString(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        if (matches!(self.k, KindBinaryLiteral | KindBytes)
            || self.k == KindString && self.Collation() == charset::CollationBin)
            && IsBinaryStr(target)
        {
            let mut bytes = self.GetBytes();
            let flen = target.GetFlen();
            if flen >= 0 && bytes.len() > flen as usize {
                let error = ErrDataTooLong.FastGen(
                    "Data Too Long, field len %d, data len %d",
                    &[flen.into(), (bytes.len() as i32).into()],
                );
                bytes.truncate(flen as usize);
                if !ctx.Flags().IgnoreTruncateErr() && !ctx.Flags().TruncateAsWarning() {
                    return Err(errors::Trace(context_error(&error).into()));
                }
                if ctx.Flags().TruncateAsWarning() {
                    ctx.AppendWarning(context_error(&error));
                }
            }
            if flen >= 0
                && target.GetType() == mysql::TypeString
                && IsBinaryStr(target)
                && bytes.len() < flen as usize
            {
                bytes.resize(flen as usize, 0);
            }
            let mut ret = Datum::default();
            ret.SetBytes(bytes);
            ret.collation = target.GetCollate().to_owned();
            return Ok(ret);
        }
        let mut err: Option<errors::Error> = None;
        let mut s = match self.k {
            KindInt64 => self.GetInt64().to_string(),
            KindUint64 => self.GetUint64().to_string(),
            KindFloat32 => strconv::FormatFloat(self.GetFloat64(), b'f', -1, 32).to_string(),
            KindFloat64 => strconv::FormatFloat(self.GetFloat64(), b'f', -1, 64).to_string(),
            KindString | KindBytes => {
                let fromBinary = self.Collation() == charset::CollationBin;
                let toBinary = target.GetCharset() == charset::CharsetBin;
                if fromBinary && toBinary {
                    self.GetString()
                } else if fromBinary {
                    self.GetBinaryStringDecoded(ctx.Flags(), &target.GetCharset())?
                } else if toBinary {
                    self.GetBinaryStringEncoded()
                } else {
                    self.GetStringWithCheck(ctx.Flags(), &target.GetCharset())?
                }
            }
            KindMysqlTime => self.GetMysqlTime().String(),
            KindMysqlDuration => self.GetMysqlDuration().String(),
            KindMysqlDecimal => self.GetMysqlDecimal().String(),
            KindMysqlEnum => self.GetMysqlEnum().String(),
            KindMysqlSet => self.GetMysqlSet().String(),
            KindBinaryLiteral => self.GetBinaryStringDecoded(ctx.Flags(), &target.GetCharset())?,
            KindMysqlBit => {
                // BIT 转字符串时 Go 优先转 uint，失败才回退到 bit 字面量。
                match self.GetBinaryLiteral().ToInt(ctx.clone()) {
                    Ok(val) => val.to_string(),
                    Err(_) => self.GetBinaryLiteral().ToString(),
                }
            }
            KindMysqlJSON => self.GetMysqlJSON().String(),
            KindVectorFloat32 => self.GetVectorFloat32().String(),
            _ => return invalidConv(self, target.GetType()),
        };
        let (produced, err1) = ProduceStrWithSpecifiedTp(s, target, ctx.clone(), true);
        s = produced;
        if err1.is_some() {
            err = err1;
        }
        let mut ret = Datum::default();
        ret.SetString(s, target.GetCollate().to_owned());
        if target.GetCharset() == charset::CharsetBin {
            ret.k = KindBytes;
        }
        match err {
            Some(err) => Err(errors::Trace(err)),
            None => Ok(ret),
        }
    }

    // convertToInt 对应 Go 的有符号整数目标转换。
    pub fn convertToInt(&self, ctx: Context, target: &FieldType) -> Result<Datum, errors::Error> {
        let i64_value = self.toSignedInteger(ctx, target.GetType())?;
        Ok(NewIntDatum(i64_value))
    }

    // convertToUint 对应 Go 的无符号整数目标转换。
    pub fn convertToUint(&self, ctx: Context, target: &FieldType) -> Result<Datum, errors::Error> {
        let tp = target.GetType();
        let upperBound = IntegerUnsignedUpperBound(tp);
        let mut err: Option<errors::Error> = None;
        let val = match self.k {
            KindInt64 => ConvertIntToUint(ctx.Flags(), self.GetInt64(), upperBound, tp)?,
            KindUint64 => ConvertUintToUint(self.GetUint64(), upperBound, tp)?,
            KindFloat32 | KindFloat64 => {
                ConvertFloatToUint(ctx.Flags(), self.GetFloat64(), upperBound, tp)?
            }
            KindString | KindBytes => {
                let parsed = StrToUint(ctx.clone(), &self.GetString(), false)?;
                ConvertUintToUint(parsed, upperBound, tp)?
            }
            KindMysqlTime => {
                let dec = self.GetMysqlTime().ToNumber();
                let ival = rust_decimal_to_i64(dec);
                ConvertIntToUint(ctx.Flags(), ival, upperBound, tp)?
            }
            KindMysqlDuration => {
                let dec = mydecimal_from_rust_decimal(self.GetMysqlDuration().ToNumber())?;
                match ConvertDecimalToUint(&dec, upperBound, tp) {
                    Ok(value) => value,
                    Err(error) => {
                        err = Some(error.error.into());
                        error.value
                    }
                }
            }
            KindMysqlDecimal => {
                match ConvertDecimalToUint(&self.GetMysqlDecimal(), upperBound, tp) {
                    Ok(value) => value,
                    Err(error) => {
                        err = Some(error.error.into());
                        error.value
                    }
                }
            }
            KindMysqlEnum => {
                ConvertFloatToUint(ctx.Flags(), self.GetMysqlEnum().ToNumber(), upperBound, tp)?
            }
            KindMysqlSet => {
                ConvertFloatToUint(ctx.Flags(), self.GetMysqlSet().ToNumber(), upperBound, tp)?
            }
            KindBinaryLiteral | KindMysqlBit => {
                let val = self.GetBinaryLiteral().ToInt(ctx.clone())?;
                ConvertUintToUint(val, upperBound, tp)?
            }
            KindMysqlJSON => ConvertJSONToInt(ctx.clone(), self.GetMysqlJSON(), true, tp)? as u64,
            _ => return invalidConv(self, target.GetType()),
        };
        let mut ret = Datum::default();
        ret.SetUint64(val);
        match err {
            Some(err) => Err(errors::Trace(err)),
            None => Ok(ret),
        }
    }

    // convertToMysqlTimestamp 对应 Go 的 TIMESTAMP 目标转换。
    pub fn convertToMysqlTimestamp(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let mut fsp = DefaultFsp;
        if target.GetDecimal() != UnspecifiedLength as isize {
            fsp = target.GetDecimal() as i32;
        }
        let mut t = Time::default();
        let mut ret = Datum::default();
        let err = match self.k {
            KindMysqlTime => {
                let converted = self
                    .GetMysqlTime()
                    .Convert(&time_context(&ctx), target.GetType());
                match converted {
                    Ok(v) => {
                        t = v.RoundFrac(&time_context(&ctx), fsp)?;
                        None
                    }
                    Err(_) => {
                        t = self.GetMysqlTime();
                        Some(
                            ErrWrongValue
                                .GenWithStackByArgs(&[TimestampStr.into(), t.String().into()])
                                .into(),
                        )
                    }
                }
            }
            KindMysqlDuration => match self
                .GetMysqlDuration()
                .ConvertToTime(&time_context(&ctx), mysql::TypeTimestamp)
            {
                Ok(v) => {
                    t = v.RoundFrac(&time_context(&ctx), fsp)?;
                    None
                }
                Err(e) => Some(e.into()),
            },
            KindString | KindBytes => match ParseTime(
                &time_context(&ctx),
                &self.GetString(),
                mysql::TypeTimestamp,
                fsp,
            ) {
                Ok(v) => {
                    t = v;
                    None
                }
                Err(e) => Some(e.into()),
            },
            KindInt64 => match ParseTimeFromNum(
                &time_context(&ctx),
                self.GetInt64(),
                mysql::TypeTimestamp,
                fsp,
            ) {
                Ok(v) => {
                    t = v;
                    None
                }
                Err(e) => Some(e.into()),
            },
            KindMysqlDecimal => match ParseTimeFromFloatString(
                &time_context(&ctx),
                &self.GetMysqlDecimal().String(),
                mysql::TypeTimestamp,
                fsp,
            ) {
                Ok(v) => {
                    t = v;
                    None
                }
                Err(e) => Some(e.into()),
            },
            KindMysqlJSON => {
                let s = json_unquote(&self.GetMysqlJSON());
                match ParseTime(&time_context(&ctx), &s, mysql::TypeTimestamp, fsp) {
                    Ok(v) => {
                        t = v;
                        None
                    }
                    Err(e) => Some(e.into()),
                }
            }
            _ => return invalidConv(self, mysql::TypeTimestamp),
        };
        t.SetType(mysql::TypeTimestamp);
        ret.SetMysqlTime(t);
        match err {
            Some(err) => Err(errors::Trace(err)),
            None => Ok(ret),
        }
    }

    // convertToMysqlTime 对应 Go 的 DATE/DATETIME 目标转换。
    pub fn convertToMysqlTime(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let tp = target.GetType();
        let mut fsp = DefaultFsp;
        if target.GetDecimal() != UnspecifiedLength as isize {
            fsp = target.GetDecimal() as i32;
        }
        let mut t = match self.k {
            KindMysqlTime => self
                .GetMysqlTime()
                .Convert(&time_context(&ctx), tp)?
                .RoundFrac(&time_context(&ctx), fsp)?,
            KindMysqlDuration => self
                .GetMysqlDuration()
                .ConvertToTime(&time_context(&ctx), tp)?
                .RoundFrac(&time_context(&ctx), fsp)?,
            KindMysqlDecimal => ParseTimeFromFloatString(
                &time_context(&ctx),
                &self.GetMysqlDecimal().String(),
                tp,
                fsp,
            )?,
            KindString | KindBytes => ParseTime(&time_context(&ctx), &self.GetString(), tp, fsp)?,
            KindInt64 => ParseTimeFromNum(&time_context(&ctx), self.GetInt64(), tp, fsp)?,
            KindUint64 => {
                if self.GetInt64() < 0 {
                    Time::default()
                } else {
                    ParseTimeFromNum(&time_context(&ctx), self.GetInt64(), tp, fsp)?
                }
            }
            KindMysqlJSON => {
                let s = json_unquote(&self.GetMysqlJSON());
                ParseTime(&time_context(&ctx), &s, tp, fsp)?
            }
            _ => return invalidConv(self, tp),
        };
        if tp == mysql::TypeDate {
            // TypeDate 需要截断时分秒。
            t.SetCoreTime(FromDate(t.Year(), t.Month(), t.Day(), 0, 0, 0, 0));
        }
        let mut ret = Datum::default();
        ret.SetMysqlTime(t);
        Ok(ret)
    }

    // convertToMysqlDuration 对应 Go 的 TIME 目标转换。
    pub fn convertToMysqlDuration(
        &self,
        typeCtx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let tp = target.GetType();
        let mut fsp = DefaultFsp;
        if target.GetDecimal() != UnspecifiedLength as isize {
            fsp = target.GetDecimal() as i32;
        }
        let mut ret = Datum::default();
        match self.k {
            KindMysqlTime => {
                let dur = self
                    .GetMysqlTime()
                    .ConvertToDuration()?
                    .RoundFrac(fsp, typeCtx.Location())?;
                ret.SetMysqlDuration(dur);
            }
            KindMysqlDuration => {
                let dur = self.GetMysqlDuration().RoundFrac(fsp, typeCtx.Location())?;
                ret.SetMysqlDuration(dur);
            }
            KindInt64 | KindUint64 | KindFloat32 | KindFloat64 | KindMysqlDecimal => {
                let timeStr = self.ToString()?;
                let timeNum = self.ToInt64(typeCtx.clone())?;
                if timeNum > MaxDuration && timeNum < 10000000000 {
                    ret.SetMysqlDuration(Duration {
                        Duration: MaxTime,
                        Fsp: 0,
                    });
                    return Err(ErrWrongValue
                        .GenWithStackByArgs(&[TimeStr.into(), timeStr.into()])
                        .into());
                }
                if timeNum < -MaxDuration {
                    return Err(ErrWrongValue
                        .GenWithStackByArgs(&[TimeStr.into(), timeStr.into()])
                        .into());
                }
                let (t, _) = ParseDuration(&time_context(&typeCtx), &timeStr, fsp)?;
                ret.SetMysqlDuration(t);
            }
            KindString | KindBytes => {
                let (t, _) = ParseDuration(&time_context(&typeCtx), &self.GetString(), fsp)?;
                ret.SetMysqlDuration(t);
            }
            KindMysqlJSON => {
                let s = json_unquote(&self.GetMysqlJSON());
                let (t, _) = ParseDuration(&time_context(&typeCtx), &s, fsp)?;
                ret.SetMysqlDuration(t);
            }
            _ => return invalidConv(self, tp),
        }
        Ok(ret)
    }

    // convertToMysqlDecimal 对应 Go 的 decimal 目标转换和 unsigned 负数裁剪。
    pub fn convertToMysqlDecimal(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let mut ret = Datum::default();
        ret.SetLength(target.GetFlen() as i32);
        ret.SetFrac(target.GetDecimal() as i32);
        let mut dec = MyDecimal::default();
        let mut err: Option<errors::Error> = None;
        match self.k {
            KindInt64 => {
                dec.FromInt(self.GetInt64());
            }
            KindUint64 => {
                dec.FromUint(self.GetUint64());
            }
            KindFloat32 | KindFloat64 => {
                err = dec.FromFloat64(self.GetFloat64()).err().map(Into::into)
            }
            KindString | KindBytes => err = dec.FromString(&self.GetBytes()).err().map(Into::into),
            KindMysqlDecimal => dec = self.GetMysqlDecimal(),
            KindMysqlTime => dec = mydecimal_from_rust_decimal(self.GetMysqlTime().ToNumber())?,
            KindMysqlDuration => {
                dec = mydecimal_from_rust_decimal(self.GetMysqlDuration().ToNumber())?
            }
            KindMysqlEnum => {
                err = dec
                    .FromFloat64(self.GetMysqlEnum().ToNumber())
                    .err()
                    .map(Into::into)
            }
            KindMysqlSet => {
                err = dec
                    .FromFloat64(self.GetMysqlSet().ToNumber())
                    .err()
                    .map(Into::into)
            }
            KindBinaryLiteral | KindMysqlBit => {
                let val = self.GetBinaryLiteral().ToInt(ctx.clone())?;
                dec.FromUint(val);
            }
            KindMysqlJSON => dec = ConvertJSONToDecimal(ctx.clone(), self.GetMysqlJSON())?,
            _ => return invalidConv(self, target.GetType()),
        }
        let (dec1, err1) = ProduceDecWithSpecifiedTp(ctx, dec.clone(), target);
        if let Some(new_dec) = dec1 {
            dec = new_dec;
        }
        if err.is_none() {
            err = err1;
        }
        if dec.IsNegative() && mysql::HasUnsignedFlag(target.GetFlag()) {
            dec = MyDecimal::default();
            if err.is_none() {
                err = Some(
                    ErrOverflow
                        .GenWithStackByArgs(&[
                            "DECIMAL".into(),
                            format!("({}, {})", target.GetFlen(), target.GetDecimal()).into(),
                        ])
                        .into(),
                );
            }
        }
        ret.SetMysqlDecimal(dec);
        match err {
            Some(err) => Err(err),
            None => Ok(ret),
        }
    }

    // 将 Datum 转为 MySQL YEAR。
    // ConvertToMysqlYear converts a datum to MySQLYear.
    pub fn ConvertToMysqlYear(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let mut ret = Datum::default();
        let mut adjust = false;
        let y = match self.k {
            KindString | KindBytes => {
                let s = self.GetString();
                let trimS = s.trim().to_string();
                let y = StrToInt(ctx.clone(), &trimS, false)?;
                if s.len() != 4 && y == 0 && trimS.starts_with('0') {
                    adjust = true;
                }
                y
            }
            KindMysqlTime => self.GetMysqlTime().Year() as i64,
            KindMysqlDuration => self.GetMysqlDuration().ConvertToYear(&time_context(&ctx))?,
            KindMysqlJSON => ConvertJSONToInt64(ctx.clone(), self.GetMysqlJSON(), false)?,
            _ => {
                let ret = self.convertToInt(ctx.clone(), &NewFieldType(mysql::TypeLonglong))?;
                ret.GetInt64()
            }
        };
        let y = if self.k != KindMysqlDuration {
            AdjustYear(y, adjust)?
        } else {
            y
        };
        ret.SetInt64(y);
        Ok(ret)
    }

    // convertToMysqlBit 对应 Go 的 BIT 目标转换，并按 flen 裁剪。
    pub fn convertToMysqlBit(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let mut err: Option<errors::Error> = None;
        let mut uintValue = match self.k {
            KindString | KindBytes => self.GetBinaryLiteral().ToInt(ctx.clone())?,
            KindInt64 => self.GetUint64(),
            _ => self.convertToUint(ctx.clone(), target)?.GetUint64(),
        };
        if target.GetFlen() <= 0 || target.GetFlen() >= 128 {
            return Err(errors::Trace(
                ErrDataTooLong
                    .GenWithStack("Data Too Long, field len %d", &[target.GetFlen().into()])
                    .into(),
            ));
        }
        if target.GetFlen() < 64 && uintValue >= (1_u64 << target.GetFlen()) {
            uintValue = (1_u64 << target.GetFlen()) - 1;
            err = Some(
                ErrDataTooLong
                    .GenWithStack("Data Too Long, field len %d", &[target.GetFlen().into()])
                    .into(),
            );
        }
        let byteSize = (target.GetFlen() + 7) >> 3;
        let mut ret = Datum::default();
        ret.SetMysqlBit(NewBinaryLiteralFromUint(uintValue, byteSize));
        match err {
            Some(err) => Err(errors::Trace(err)),
            None => Ok(ret),
        }
    }

    // convertToMysqlEnum 对应 Go 的 ENUM 目标转换。
    pub fn convertToMysqlEnum(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let e = match self.k {
            KindString | KindBytes | KindBinaryLiteral => {
                ParseEnum(target.GetElems(), &self.GetString(), target.GetCollate())?
            }
            KindMysqlEnum => {
                if self.i == 0 {
                    Enum::default()
                } else {
                    ParseEnum(
                        target.GetElems(),
                        &self.GetMysqlEnum().Name,
                        target.GetCollate(),
                    )?
                }
            }
            KindMysqlSet => ParseEnum(
                target.GetElems(),
                &self.GetMysqlSet().Name,
                target.GetCollate(),
            )?,
            _ => {
                let uintDatum = self.convertToUint(ctx, target)?;
                ParseEnumValue(target.GetElems(), uintDatum.GetUint64())?
            }
        };
        let mut ret = Datum::default();
        ret.SetMysqlEnum(e, target.GetCollate().to_owned());
        Ok(ret)
    }

    // convertToMysqlSet 对应 Go 的 SET 目标转换。
    pub fn convertToMysqlSet(
        &self,
        ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        let s = match self.k {
            KindString | KindBytes | KindBinaryLiteral => {
                ParseSet(target.GetElems(), &self.GetString(), target.GetCollate())?
            }
            KindMysqlEnum => ParseSet(
                target.GetElems(),
                &self.GetMysqlEnum().Name,
                target.GetCollate(),
            )?,
            KindMysqlSet => ParseSet(
                target.GetElems(),
                &self.GetMysqlSet().Name,
                target.GetCollate(),
            )?,
            KindVectorFloat32 => return invalidConv(self, mysql::TypeSet),
            _ => {
                let uintDatum = self.convertToUint(ctx, target)?;
                ParseSetValue(target.GetElems(), uintDatum.GetUint64())?
            }
        };
        let mut ret = Datum::default();
        ret.SetMysqlSet(s, target.GetCollate().to_owned());
        Ok(ret)
    }

    // convertToMysqlJSON 对应 Go 的 JSON 目标转换；字符串路径解析 JSON，数值路径 CreateBinaryJSON。
    pub fn convertToMysqlJSON(&self, _target: &FieldType) -> Result<Datum, errors::Error> {
        let mut ret = Datum::default();
        match self.k {
            KindString | KindBytes => {
                ret.SetMysqlJSON(ParseBinaryJSONFromString(&self.GetString())?)
            }
            KindMysqlSet | KindMysqlEnum => {
                let s = self.ToString()?;
                ret.SetMysqlJSON(ParseBinaryJSONFromString(&s)?);
            }
            KindInt64 => ret.SetMysqlJSON(CreateBinaryJSON(self.GetInt64())),
            KindUint64 => ret.SetMysqlJSON(CreateBinaryJSON(self.GetUint64())),
            KindFloat32 | KindFloat64 => ret.SetMysqlJSON(CreateBinaryJSON(self.GetFloat64())),
            KindMysqlDecimal => {
                ret.SetMysqlJSON(CreateBinaryJSON(self.GetMysqlDecimal().ToFloat64()?))
            }
            KindMysqlJSON => ret = self.clone(),
            KindMysqlTime => ret.SetMysqlJSON(CreateBinaryJSONFromTime(self.GetMysqlTime())),
            KindMysqlDuration => {
                ret.SetMysqlJSON(CreateBinaryJSONFromDuration(self.GetMysqlDuration()))
            }
            KindBinaryLiteral => {
                return Err(JsonError::new(
                    ErrInvalidJSONCharset,
                    format!("invalid JSON charset {}", charset::CharsetBin),
                )
                .into());
            }
            _ => ret.SetMysqlJSON(CreateBinaryJSON(self.ToString()?)),
        }
        Ok(ret)
    }

    // convertToVectorFloat32 对应 Go 的 vector 目标转换和维度检查。
    pub fn convertToVectorFloat32(
        &self,
        _ctx: Context,
        target: &FieldType,
    ) -> Result<Datum, errors::Error> {
        match self.k {
            KindVectorFloat32 => {
                let v = self.GetVectorFloat32();
                v.CheckDimsFitColumn(target.GetFlen() as i32)?;
                Ok(self.clone())
            }
            KindString | KindBytes => {
                let v = ParseVectorFloat32(&self.GetString())?;
                v.CheckDimsFitColumn(target.GetFlen() as i32)?;
                let mut ret = Datum::default();
                ret.SetVectorFloat32(v);
                Ok(ret)
            }
            _ => invalidConv(self, mysql::TypeTiDBVectorFloat32),
        }
    }

    // ToBool converts to a bool. We will use 1 for true, and 0 for false.
    // 转为布尔语义的 0/1。
    pub fn ToBool(&self, ctx: Context) -> Result<i64, errors::Error> {
        let isZero = match self.Kind() {
            KindInt64 => self.GetInt64() == 0,
            KindUint64 => self.GetUint64() == 0,
            KindFloat32 | KindFloat64 => self.GetFloat64() == 0.0,
            KindString | KindBytes => StrToFloat(ctx.clone(), &self.GetString(), false)? == 0.0,
            KindMysqlTime => self.GetMysqlTime().IsZero(),
            KindMysqlDuration => self.GetMysqlDuration().Duration == 0,
            KindMysqlDecimal => self.GetMysqlDecimal().IsZero(),
            KindMysqlEnum => self.GetMysqlEnum().ToNumber() == 0.0,
            KindMysqlSet => self.GetMysqlSet().ToNumber() == 0.0,
            KindBinaryLiteral | KindMysqlBit => self.GetBinaryLiteral().ToInt(ctx.clone())? == 0,
            KindMysqlJSON => self.GetMysqlJSON().IsZero(),
            KindVectorFloat32 => self.GetVectorFloat32().IsZeroValue(),
            _ => {
                return Err(errors::Errorf(format!(
                    "cannot convert {:?} to bool",
                    self.GetValue()
                )));
            }
        };
        Ok(if isZero { 0 } else { 1 })
    }

    // 转为 MyDecimal。
    // ToDecimal converts to a decimal.
    pub fn ToDecimal(&self, ctx: Context) -> Result<MyDecimal, errors::Error> {
        match self.Kind() {
            KindMysqlTime => mydecimal_from_rust_decimal(self.GetMysqlTime().ToNumber()),
            KindMysqlDuration => mydecimal_from_rust_decimal(self.GetMysqlDuration().ToNumber()),
            _ => ConvertDatumToDecimal(ctx, self.clone()),
        }
    }

    // ToInt64 converts to a int64.
    // 转为有符号 64 位整数。
    pub fn ToInt64(&self, ctx: Context) -> Result<i64, errors::Error> {
        if self.Kind() == KindMysqlBit {
            return Ok(self.GetBinaryLiteral().ToInt(ctx)? as i64);
        }
        self.toSignedInteger(ctx, mysql::TypeLonglong)
    }

    // toSignedInteger 对应 Go 的内部有符号整数转换。
    pub fn toSignedInteger(&self, ctx: Context, tp: u8) -> Result<i64, errors::Error> {
        let lowerBound = IntegerSignedLowerBound(tp);
        let upperBound = IntegerSignedUpperBound(tp);
        match self.Kind() {
            KindInt64 => {
                ConvertIntToInt(self.GetInt64(), lowerBound, upperBound, tp).map_err(Into::into)
            }
            KindUint64 => ConvertUintToInt(self.GetUint64(), upperBound, tp).map_err(Into::into),
            KindFloat32 => ConvertFloatToInt(self.GetFloat32() as f64, lowerBound, upperBound, tp)
                .map_err(Into::into),
            KindFloat64 => {
                ConvertFloatToInt(self.GetFloat64(), lowerBound, upperBound, tp).map_err(Into::into)
            }
            KindString | KindBytes => {
                let iVal = StrToInt(ctx.clone(), &self.GetString(), false)?;
                ConvertIntToInt(iVal, lowerBound, upperBound, tp).map_err(Into::into)
            }
            KindMysqlTime => {
                let t = self
                    .GetMysqlTime()
                    .RoundFrac(&time_context(&ctx), DefaultFsp)?;
                let ival = rust_decimal_to_i64(t.ToNumber());
                ConvertIntToInt(ival, lowerBound, upperBound, tp).map_err(Into::into)
            }
            KindMysqlDuration => {
                let dur = self
                    .GetMysqlDuration()
                    .RoundFrac(DefaultFsp, ctx.Location())?;
                let ival = rust_decimal_to_i64(dur.ToNumber());
                ConvertIntToInt(ival, lowerBound, upperBound, tp).map_err(Into::into)
            }
            KindMysqlDecimal => {
                let mut to = MyDecimal::default();
                self.GetMysqlDecimal().Round(&mut to, 0, ModeHalfUp)?;
                let (ival, conversion) = to.ToInt();
                conversion?;
                ConvertIntToInt(ival, lowerBound, upperBound, tp).map_err(Into::into)
            }
            KindMysqlEnum => {
                ConvertFloatToInt(self.GetMysqlEnum().ToNumber(), lowerBound, upperBound, tp)
                    .map_err(Into::into)
            }
            KindMysqlSet => {
                ConvertFloatToInt(self.GetMysqlSet().ToNumber(), lowerBound, upperBound, tp)
                    .map_err(Into::into)
            }
            KindMysqlJSON => Ok(ConvertJSONToInt(ctx, self.GetMysqlJSON(), false, tp)?),
            KindBinaryLiteral | KindMysqlBit => {
                let val = self.GetBinaryLiteral().ToInt(ctx)?;
                ConvertUintToInt(val, upperBound, tp).map_err(Into::into)
            }
            _ => Err(errors::Errorf(format!(
                "cannot convert {:?} to int64",
                self.GetValue()
            ))),
        }
    }

    // ToFloat64 converts to a float64.
    // 转为 float64。
    pub fn ToFloat64(&self, ctx: Context) -> Result<f64, errors::Error> {
        match self.Kind() {
            KindInt64 => Ok(self.GetInt64() as f64),
            KindUint64 => Ok(self.GetUint64() as f64),
            KindFloat32 => Ok(self.GetFloat32() as f64),
            KindFloat64 => Ok(self.GetFloat64()),
            KindString | KindBytes => StrToFloat(ctx, &self.GetString(), false).map_err(Into::into),
            KindMysqlTime => Ok(rust_decimal_to_f64(self.GetMysqlTime().ToNumber())),
            KindMysqlDuration => Ok(rust_decimal_to_f64(self.GetMysqlDuration().ToNumber())),
            KindMysqlDecimal => self.GetMysqlDecimal().ToFloat64().map_err(Into::into),
            KindMysqlEnum => Ok(self.GetMysqlEnum().ToNumber()),
            KindMysqlSet => Ok(self.GetMysqlSet().ToNumber()),
            KindBinaryLiteral | KindMysqlBit => Ok(self.GetBinaryLiteral().ToInt(ctx)? as f64),
            KindMysqlJSON => Ok(ConvertJSONToFloat(ctx, self.GetMysqlJSON())?),
            _ => Err(errors::Errorf(format!(
                "cannot convert {:?} to float64",
                self.GetValue()
            ))),
        }
    }

    // ToString gets the string representation of the datum.
    // 转为字符串。
    pub fn ToString(&self) -> Result<String, errors::Error> {
        match self.Kind() {
            KindInt64 => Ok(self.GetInt64().to_string()),
            KindUint64 => Ok(self.GetUint64().to_string()),
            KindFloat32 => {
                Ok(strconv::FormatFloat(self.GetFloat32() as f64, b'f', -1, 32).to_string())
            }
            KindFloat64 => Ok(strconv::FormatFloat(self.GetFloat64(), b'f', -1, 64).to_string()),
            KindString | KindBytes => Ok(self.GetString()),
            KindMysqlTime => Ok(self.GetMysqlTime().String()),
            KindMysqlDuration => Ok(self.GetMysqlDuration().String()),
            KindMysqlDecimal => Ok(self.GetMysqlDecimal().String()),
            KindMysqlEnum => Ok(self.GetMysqlEnum().String()),
            KindMysqlSet => Ok(self.GetMysqlSet().String()),
            KindMysqlJSON => Ok(self.GetMysqlJSON().String()),
            KindBinaryLiteral | KindMysqlBit => Ok(self.GetBinaryLiteral().ToString()),
            KindVectorFloat32 => Ok(self.GetVectorFloat32().String()),
            KindNull => Ok(String::new()),
            _ => Err(errors::Errorf(format!(
                "cannot convert {:?} to string",
                self.GetValue()
            ))),
        }
    }

    // ToBytes gets the bytes representation of the datum.
    // 转为字节。
    pub fn ToBytes(&self) -> Result<Vec<u8>, errors::Error> {
        match self.k {
            KindString | KindBytes => Ok(self.GetBytes()),
            _ => Ok(self.ToString()?.into_bytes()),
        }
    }

    // 按校对规则得到用于哈希/索引的字节键。
    // ToHashKey gets the bytes representation of the datum considering collation.
    pub fn ToHashKey(&self) -> Result<Vec<u8>, errors::Error> {
        match self.k {
            KindString | KindBytes => {
                Ok(collate::GetCollator(&self.Collation()).Key(&self.GetString()))
            }
            _ => {
                let str_value = self.ToString()?;
                Ok(collate::GetCollator(&self.Collation()).Key(&str_value))
            }
        }
    }

    // ToMysqlJSON is similar to convertToMysqlJSON except that string is used as primitive.
    // 转为 BinaryJSON。
    pub fn ToMysqlJSON(&self) -> Result<BinaryJSON, errors::Error> {
        match self.Kind() {
            KindMysqlJSON => Ok(self.GetMysqlJSON()),
            KindInt64 => Ok(CreateBinaryJSON(self.GetInt64())),
            KindUint64 => Ok(CreateBinaryJSON(self.GetUint64())),
            KindFloat32 | KindFloat64 => Ok(CreateBinaryJSON(self.GetFloat64())),
            KindMysqlDecimal => Ok(CreateBinaryJSON(self.GetMysqlDecimal().ToFloat64()?)),
            KindString | KindBytes => Ok(CreateBinaryJSON(self.GetString())),
            KindBinaryLiteral | KindMysqlBit => {
                Ok(CreateBinaryJSON(self.GetBinaryLiteral().ToString()))
            }
            KindNull => Ok(CreateBinaryJSON(())),
            KindMysqlTime => Ok(CreateBinaryJSONFromTime(self.GetMysqlTime())),
            KindMysqlDuration => Ok(CreateBinaryJSONFromDuration(self.GetMysqlDuration())),
            _ => Ok(CreateBinaryJSON(self.ToString()?)),
        }
    }

    // 估算本 Datum 内存占用。
    // MemUsage gets the memory usage of datum.
    pub fn MemUsage(&self) -> i64 {
        EmptyDatumSize + self.b.capacity() as i64 + self.collation.len() as i64
    }

    // MarshalJSON implements Marshaler.MarshalJSON interface.
    // JSON 序列化。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, errors::Error> {
        use base64::Engine;

        let jd = jsonDatum::from_datum(self)?;
        let mut value = serde_json::Map::new();
        value.insert("k".to_owned(), serde_json::Value::from(jd.K));
        if jd.Decimal != 0 {
            value.insert("decimal".to_owned(), serde_json::Value::from(jd.Decimal));
        }
        if jd.Length != 0 {
            value.insert("length".to_owned(), serde_json::Value::from(jd.Length));
        }
        if jd.I != 0 {
            value.insert("i".to_owned(), serde_json::Value::from(jd.I));
        }
        if !jd.Collation.is_empty() {
            value.insert(
                "collation".to_owned(),
                serde_json::Value::from(jd.Collation),
            );
        }
        if !jd.B.is_empty() {
            value.insert(
                "b".to_owned(),
                serde_json::Value::from(base64::engine::general_purpose::STANDARD.encode(jd.B)),
            );
        }
        value.insert(
            "time".to_owned(),
            serde_json::Value::from(jd.Time.coreTime.0),
        );
        if jd.K == KindMysqlDecimal {
            let decimal_json = jd.MyDecimal.MarshalJSON()?;
            value.insert(
                "mydecimal".to_owned(),
                serde_json::from_slice(&decimal_json)?,
            );
        }
        Ok(serde_json::to_vec(&serde_json::Value::Object(value))?)
    }

    // UnmarshalJSON implements Unmarshaler.UnmarshalJSON interface.
    // JSON 反序列化。
    pub fn UnmarshalJSON(&mut self, data: &[u8]) -> Result<(), errors::Error> {
        use base64::Engine;

        let value: serde_json::Value = serde_json::from_slice(data)?;
        let object = value
            .as_object()
            .ok_or_else(|| errors::New("datum JSON must be an object"))?;
        self.k = object
            .get("k")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u8;
        self.decimal = object
            .get("decimal")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u16;
        self.length = object
            .get("length")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        self.i = object
            .get("i")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        self.collation = object
            .get("collation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        self.b = match object.get("b") {
            Some(serde_json::Value::String(encoded)) => {
                base64::engine::general_purpose::STANDARD.decode(encoded)?
            }
            Some(serde_json::Value::Array(bytes)) => bytes
                .iter()
                .filter_map(serde_json::Value::as_u64)
                .map(|byte| byte as u8)
                .collect(),
            _ => Vec::new(),
        };
        match self.k {
            KindMysqlTime => {
                let core_time = object
                    .get("time")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default();
                self.SetMysqlTime(Time {
                    coreTime: CoreTime(core_time),
                });
            }
            KindMysqlDecimal => {
                let mut decimal = MyDecimal::default();
                if let Some(value) = object.get("mydecimal") {
                    decimal.UnmarshalJSON(&serde_json::to_vec(value)?)?;
                }
                self.SetMysqlDecimal(decimal);
            }
            _ => {}
        }
        Ok(())
    }

    // EstimatedMemUsage returns the estimated bytes consumed of a Datum.
    // 估算多行 Datum 内存占用。
    pub fn EstimatedMemUsage(&self) -> i64 {
        let mut bytesConsumed = sizeOfEmptyDatum;
        match self.Kind() {
            KindMysqlDecimal => bytesConsumed += sizeOfMyDecimal,
            KindMysqlTime => bytesConsumed += sizeOfMysqlTime,
            KindVectorFloat32 => {
                bytesConsumed += self.GetVectorFloat32().EstimatedMemUsage() as i32
            }
            _ => bytesConsumed += self.b.len() as i32,
        }
        bytesConsumed as i64
    }
}

// ProduceFloatWithSpecifiedTp produces a new float64 according to `flen` and `decimal`.
// 按目标 FieldType 裁剪/检查浮点。
pub fn ProduceFloatWithSpecifiedTp(f: f64, target: &FieldType) -> (f64, Option<errors::Error>) {
    if f.is_nan() {
        return (0.0, Some(overflow(&f, target.GetType())));
    }
    if f.is_infinite() {
        return (f, Some(overflow(&f, target.GetType())));
    }
    let mut out = f;
    let mut err = None;
    if target.GetFlen() != UnspecifiedLength as isize
        && target.GetDecimal() != UnspecifiedLength as isize
    {
        let (value, truncation) =
            TruncateFloat(out, target.GetFlen() as i32, target.GetDecimal() as i32);
        out = value;
        err = truncation.map(Into::into);
    }
    if mysql::HasUnsignedFlag(target.GetFlag()) && out < 0.0 {
        return (0.0, Some(overflow(&out, target.GetType())));
    }
    if target.GetType() == mysql::TypeFloat && (out > math::MaxFloat32 || out < -math::MaxFloat32) {
        if out > 0.0 {
            return (math::MaxFloat32, Some(overflow(&out, target.GetType())));
        }
        return (-math::MaxFloat32, Some(overflow(&out, target.GetType())));
    }
    (out, err.map(errors::Trace))
}

// ProduceStrWithSpecifiedTp produces a new string according to `flen` and `chs`.
// 按目标长度/字符集裁剪字符串。
pub fn ProduceStrWithSpecifiedTp(
    mut s: String,
    tp: &FieldType,
    mut ctx: Context,
    padZero: bool,
) -> (String, Option<errors::Error>) {
    let flen = tp.GetFlen();
    let chs = tp.GetCharset();
    let mut err = None;
    if flen >= 0 {
        let mut overflowed = String::new();
        let mut characterLen = 0_i32;
        let mut needCalculateLen = false;

        if chs != charset::CharsetBin {
            match tp.GetType() {
                mysql::TypeTinyBlob
                | mysql::TypeMediumBlob
                | mysql::TypeLongBlob
                | mysql::TypeBlob => {
                    characterLen = s.len() as i32;
                    if characterLen as isize > flen {
                        let truncateLen = utf8_safe_prefix_len(&s, flen as usize);
                        overflowed = s[truncateLen..].to_string();
                        s = truncateStr(s, truncateLen as i32);
                    }
                }
                _ => {
                    if s.len() > flen as usize {
                        characterLen = utf8::RuneCountInString(&s) as i32;
                        if characterLen as isize > flen {
                            let truncateLen = nth_rune_boundary(&s, flen as usize);
                            overflowed = s[truncateLen..].to_string();
                            s = truncateStr(s, truncateLen as i32);
                        } else {
                            needCalculateLen = true;
                        }
                    }
                }
            }
        } else if s.len() > flen as usize {
            characterLen = s.len() as i32;
            // Go strings can be sliced at any byte offset. Rust `String` cannot,
            // so preserve the same byte-length limit at the nearest representable
            // UTF-8 boundary instead of panicking on arbitrary binary data.
            let truncateLen = utf8_safe_prefix_len(&s, flen as usize);
            overflowed = s[truncateLen..].to_string();
            s = truncateStr(s, truncateLen as i32);
        }

        if !overflowed.is_empty() {
            let trimmed = overflowed.trim_end_matches([' ', '\t', '\n', '\r']);
            if trimmed.is_empty() && !IsBinaryStr(tp) && IsTypeChar(tp.GetType()) {
                if tp.GetType() == mysql::TypeVarchar {
                    if needCalculateLen {
                        characterLen = utf8::RuneCountInString(&s) as i32;
                    }
                    let warning = ErrTruncated.FastGen(
                        "Data truncated, field len %d, data len %d",
                        &[flen.into(), characterLen.into()],
                    );
                    ctx.AppendWarning(context_error(&warning));
                }
            } else {
                if needCalculateLen {
                    characterLen = utf8::RuneCountInString(&s) as i32;
                }
                err = Some(ErrDataTooLong.FastGen(
                    "Data Too Long, field len %d, data len %d",
                    &[flen.into(), characterLen.into()],
                ));
            }
        }

        if tp.GetType() == mysql::TypeString
            && IsBinaryStr(tp)
            && s.len() < flen as usize
            && padZero
        {
            s.push_str(&"\0".repeat(flen as usize - s.len()));
        }
    }
    let err = match err {
        None => None,
        Some(error) if ctx.Flags().IgnoreTruncateErr() => None,
        Some(error) if ctx.Flags().TruncateAsWarning() => {
            ctx.AppendWarning(context_error(&error));
            None
        }
        Some(error) => Some(error.into()),
    };
    (s, err.map(errors::Trace))
}

// ProduceDecWithSpecifiedTp produces a new decimal according to `flen` and `decimal`.
// 按精度与标度裁剪 DECIMAL。
pub fn ProduceDecWithSpecifiedTp(
    mut ctx: Context,
    mut dec: MyDecimal,
    tp: &FieldType,
) -> (Option<MyDecimal>, Option<errors::Error>) {
    let (flen, decimal) = (tp.GetFlen(), tp.GetDecimal());
    let mut err = None;
    if flen != UnspecifiedLength as isize && decimal != UnspecifiedLength as isize {
        if flen < decimal {
            return (
                None,
                Some(ErrMBiggerThanD.GenWithStackByArgs(&["".into()]).into()),
            );
        }
        let mut old = None;
        if dec.GetDigitsFrac() as isize > decimal {
            old = Some(dec.clone());
        }
        if dec.GetDigitsFrac() as isize != decimal {
            let original = dec.clone();
            let _ = original.Round(&mut dec, decimal, ModeHalfUp);
        }
        // Go checks the integer width after removing leading zeroes. In
        // particular, DECIMAL(M, M) accepts values in (-1, 1); the printable
        // leading zero is not an integer digit for precision accounting.
        let rendered = dec.ToString();
        let unsigned = rendered.strip_prefix(b"-").unwrap_or(&rendered);
        let integer = unsigned
            .split(|byte| *byte == b'.')
            .next()
            .unwrap_or_default();
        let digitsInt = integer.iter().skip_while(|byte| **byte == b'0').count() as isize;
        if flen - decimal < digitsInt {
            dec = NewMaxOrMinDec(dec.IsNegative(), flen as usize, decimal as usize);
            err = Some(
                ErrOverflow
                    .GenWithStackByArgs(&[
                        "DECIMAL".into(),
                        format!("({}, {})", flen, decimal).into(),
                    ])
                    .into(),
            );
        } else if let Some(old) = old {
            if dec.Compare(&old) != 0 {
                let warning = ErrTruncatedWrongVal.FastGenByArgs(&[
                    "DECIMAL".into(),
                    String::from_utf8_lossy(&old.ToString()).into_owned().into(),
                ]);
                ctx.AppendWarning(context_error(&warning));
            }
        }
    }
    if mysql::HasUnsignedFlag(tp.GetFlag()) && dec.IsNegative() {
        dec.FromUint(0);
    }
    (Some(dec), err)
}

// ConvertDatumToDecimal converts datum to decimal.
// 将 Datum 转为 MyDecimal。
pub fn ConvertDatumToDecimal(ctx: Context, d: Datum) -> Result<MyDecimal, errors::Error> {
    let mut dec = MyDecimal::default();
    match d.Kind() {
        KindInt64 => {
            dec.FromInt(d.GetInt64());
        }
        KindUint64 => {
            dec.FromUint(d.GetUint64());
        }
        KindFloat32 => dec.FromFloat64(d.GetFloat32() as f64)?,
        KindFloat64 => dec.FromFloat64(d.GetFloat64())?,
        KindString => dec.FromString(&d.GetBytes())?,
        KindMysqlDecimal => dec = d.GetMysqlDecimal(),
        KindMysqlEnum => {
            dec.FromUint(d.GetMysqlEnum().Value);
        }
        KindMysqlSet => {
            dec.FromUint(d.GetMysqlSet().Value);
        }
        KindBinaryLiteral | KindMysqlBit => {
            dec.FromUint(d.GetBinaryLiteral().ToInt(ctx)?);
        }
        KindMysqlJSON => dec = ConvertJSONToDecimal(ctx, d.GetMysqlJSON())?,
        _ => {
            return Err(errors::Errorf(format!(
                "can't convert {:?} to decimal",
                d.GetValue()
            )));
        }
    }
    Ok(dec)
}

// jsonDatum 对应 Go MarshalJSON/UnmarshalJSON 的中间结构。
pub struct jsonDatum {
    pub K: u8,
    pub Decimal: u16,
    pub Length: u32,
    pub I: i64,
    pub Collation: String,
    pub B: Vec<u8>,
    pub Time: Time,
    pub MyDecimal: MyDecimal,
}

impl jsonDatum {
    // from_datum 保留 Go MarshalJSON 中按 kind 填充 Time/MyDecimal 的逻辑。
    pub fn from_datum(d: &Datum) -> Result<jsonDatum, errors::Error> {
        let mut jd = jsonDatum {
            K: d.k,
            Decimal: d.decimal,
            Length: d.length,
            I: d.i,
            Collation: d.collation.clone(),
            B: d.b.clone(),
            Time: Time::default(),
            MyDecimal: MyDecimal::default(),
        };
        match d.k {
            KindMysqlTime => jd.Time = d.GetMysqlTime(),
            KindMysqlDecimal => jd.MyDecimal = d.GetMysqlDecimal(),
            _ => {
                if d.x.is_some() {
                    return Err(errors::Errorf(format!("unsupported type: {}", d.k)));
                }
            }
        }
        Ok(jd)
    }
}

// invalidConv 对应 Go 中统一的非法类型转换错误。
pub fn invalidConv(d: &Datum, tp: u8) -> Result<Datum, errors::Error> {
    Err(errors::Errorf(format!(
        "cannot convert datum from {} to type {}",
        KindStr(d.Kind()),
        TypeStr(tp)
    )))
}

// NewDatum creates a new Datum from an interface{}.
// 由任意值构造 Datum。
pub fn NewDatum(in_value: &dyn any::Any) -> Datum {
    let mut d = Datum::default();
    d.SetValueWithDefaultCollation(in_value);
    d
}

// NewIntDatum creates a new Datum from an int64 value.
// 构造 Int64 Datum。
pub fn NewIntDatum(i: i64) -> Datum {
    let mut d = Datum::default();
    d.SetInt64(i);
    d
}

// NewUintDatum creates a new Datum from an uint64 value.
// 构造 Uint64 Datum。
pub fn NewUintDatum(i: u64) -> Datum {
    let mut d = Datum::default();
    d.SetUint64(i);
    d
}

// NewBytesDatum creates a new Datum from a byte slice.
// 构造 Bytes Datum。
pub fn NewBytesDatum(b: Vec<u8>) -> Datum {
    let mut d = Datum::default();
    d.SetBytes(b);
    d
}

// NewStringDatum creates a new Datum from a string.
// 构造默认校对字符串 Datum。
pub fn NewStringDatum(s: String) -> Datum {
    let mut d = Datum::default();
    d.SetString(s, mysql::DefaultCollationName.to_string());
    d
}

// NewCollationStringDatum creates a new Datum from a string with collation.
// 构造带校对规则的字符串 Datum。
pub fn NewCollationStringDatum(s: String, collation: String) -> Datum {
    let mut d = Datum::default();
    d.SetString(s, collation);
    d
}

// NewFloat64Datum creates a new Datum from a float64 value.
// 构造 Float64 Datum。
pub fn NewFloat64Datum(f: f64) -> Datum {
    let mut d = Datum::default();
    d.SetFloat64(f);
    d
}

// NewFloat32Datum creates a new Datum from a float32 value.
// 构造 Float32 Datum。
pub fn NewFloat32Datum(f: f32) -> Datum {
    let mut d = Datum::default();
    d.SetFloat32(f);
    d
}

// NewDurationDatum creates a new Datum from a Duration value.
// 构造 Duration Datum。
pub fn NewDurationDatum(dur: Duration) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlDuration(dur);
    d
}

// NewTimeDatum creates a new Time from a Time value.
// 构造 Time Datum。
pub fn NewTimeDatum(t: Time) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlTime(t);
    d
}

// NewDecimalDatum creates a new Datum from a MyDecimal value.
// 构造 Decimal Datum。
pub fn NewDecimalDatum(dec: MyDecimal) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlDecimal(dec);
    d
}

// NewJSONDatum creates a new Datum from a BinaryJSON value.
// 构造 JSON Datum。
pub fn NewJSONDatum(j: BinaryJSON) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlJSON(j);
    d
}

// NewVectorFloat32Datum creates a new Datum from a VectorFloat32 value.
// 构造向量 Datum。
pub fn NewVectorFloat32Datum(v: VectorFloat32) -> Datum {
    let mut d = Datum::default();
    d.SetVectorFloat32(v);
    d
}

// NewBinaryLiteralDatum creates a new BinaryLiteral Datum for a BinaryLiteral value.
// 构造二进制字面量 Datum。
pub fn NewBinaryLiteralDatum(b: BinaryLiteral) -> Datum {
    let mut d = Datum::default();
    d.SetBinaryLiteral(b);
    d
}

// NewMysqlBitDatum creates a new MysqlBit Datum for a BinaryLiteral value.
// 构造表列 BIT Datum。
pub fn NewMysqlBitDatum(b: BinaryLiteral) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlBit(b);
    d
}

// NewMysqlEnumDatum creates a new MysqlEnum Datum for a Enum value.
// 构造 ENUM Datum。
pub fn NewMysqlEnumDatum(e: Enum) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlEnum(e, mysql::DefaultCollationName.to_string());
    d
}

// NewCollateMysqlEnumDatum create a new MysqlEnum Datum for a Enum value with collation information.
// 构造带校对的 ENUM Datum。
pub fn NewCollateMysqlEnumDatum(e: Enum, collation: String) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlEnum(e, collation);
    d
}

// NewMysqlSetDatum creates a new MysqlSet Datum for a Enum value.
// 构造 SET Datum。
pub fn NewMysqlSetDatum(e: Set, collation: String) -> Datum {
    let mut d = Datum::default();
    d.SetMysqlSet(e, collation);
    d
}

// MakeDatums creates datum slice from interfaces.
// 将任意值列表转为 Datum 切片。
pub fn MakeDatums(args: Vec<&dyn any::Any>) -> Vec<Datum> {
    args.into_iter().map(NewDatum).collect()
}

// MinNotNullDatum returns a datum represents minimum not null value.
// 最小非 NULL 哨兵 Datum。
pub fn MinNotNullDatum() -> Datum {
    Datum {
        k: KindMinNotNull,
        ..Datum::default()
    }
}

// MaxValueDatum returns a datum represents max value.
// 最大价值哨兵 Datum。
pub fn MaxValueDatum() -> Datum {
    Datum {
        k: KindMaxValue,
        ..Datum::default()
    }
}

// SortDatums sorts a slice of datum.
// 按 Compare 对 Datum 切片排序。
pub fn SortDatums(ctx: Context, datums: &mut [Datum]) -> Result<(), errors::Error> {
    let mut err: Option<errors::Error> = None;
    datums.sort_by(|a, b| {
        let collator = collate::GetCollator(&b.Collation());
        match a.Compare(ctx.clone(), b, collator.as_ref()) {
            Ok(cmp) => cmp_to_ordering(cmp),
            Err(e) => {
                err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    match err {
        Some(err) => Err(errors::Trace(err)),
        None => Ok(()),
    }
}

// isPrintable checks if a string is valid UTF-8 and contains no control characters.
// 是否全部为可打印 ASCII。
pub fn isPrintable(s: impl AsRef<[u8]>) -> bool {
    let Ok(s) = std::str::from_utf8(s.as_ref()) else {
        return false;
    };
    for r in s.chars() {
        if unicode::IsControl(r) {
            return false;
        }
    }
    true
}

// DatumsToString converts several datums to formatted string.
// 将一行 Datum 格式化为调试字符串。
pub fn DatumsToString(datums: &[Datum], handleSpecialValue: bool) -> Result<String, errors::Error> {
    datumsToString(datums, handleSpecialValue, false)
}

// DatumsToStringSmart is like DatumsToString, but with smart detection of non-printable data.
// 智能截断版 DatumsToString。
pub fn DatumsToStringSmart(
    datums: &[Datum],
    handleSpecialValue: bool,
) -> Result<String, errors::Error> {
    datumsToString(datums, handleSpecialValue, true)
}

// datumsToString 对应 Go 的格式化主逻辑，负责括号、特殊值、字符串加引号和长值截断。
pub fn datumsToString(
    datums: &[Datum],
    handleSpecialValue: bool,
    binaryAsHex: bool,
) -> Result<String, errors::Error> {
    let n = datums.len();
    let mut builder = String::with_capacity(8 * n);
    if n > 1 {
        builder.push('(');
    }
    for (i, datum) in datums.iter().enumerate() {
        if i > 0 {
            builder.push_str(", ");
        }
        if handleSpecialValue {
            match datum.Kind() {
                KindNull => {
                    builder.push_str("NULL");
                    continue;
                }
                KindMinNotNull => {
                    builder.push_str("-inf");
                    continue;
                }
                KindMaxValue => {
                    builder.push_str("+inf");
                    continue;
                }
                _ => {}
            }
        }
        let mut str_value = datum.ToString()?;
        let mut string_bytes = if datum.Kind() == KindString {
            datum.GetBytes()
        } else {
            Vec::new()
        };
        /// 日志/调试相关长度辅助。
        const logDatumLen: usize = 2048;
        let mut originalLen: isize = -1;
        let value_len = if datum.Kind() == KindString {
            string_bytes.len()
        } else {
            str_value.len()
        };
        if value_len > logDatumLen {
            originalLen = value_len as isize;
            if datum.Kind() == KindString {
                string_bytes.truncate(logDatumLen);
                str_value = String::from_utf8_lossy(&string_bytes).into_owned();
            } else {
                str_value.truncate(logDatumLen);
            }
        }
        if datum.Kind() == KindString {
            if !binaryAsHex || isPrintable(&string_bytes) {
                builder.push('"');
                builder.push_str(&str_value);
                builder.push('"');
            } else {
                builder.push_str("0x");
                for byte in &string_bytes {
                    builder.push_str(&format!("{byte:02X}"));
                }
            }
        } else {
            builder.push_str(&str_value);
        }
        if originalLen != -1 {
            builder.push_str(" len(");
            builder.push_str(&originalLen.to_string());
            builder.push(')');
        }
    }
    if n > 1 {
        builder.push(')');
    }
    Ok(builder)
}

// DatumsToStrNoErr converts some datums to a formatted string and logs errors.
// DatumsToString 忽略错误。
pub fn DatumsToStrNoErr(datums: &[Datum]) -> String {
    let (str_value, err) = match DatumsToString(datums, true) {
        Ok(v) => (v, None),
        Err(e) => (String::new(), Some(e)),
    };
    terror::Log(err.map(errors::Trace));
    str_value
}

// DatumsToStrNoErrSmart converts datums to string with non-printable detection and logs errors.
// 智能截断且忽略错误。
pub fn DatumsToStrNoErrSmart(datums: &[Datum]) -> String {
    let (str_value, err) = match DatumsToStringSmart(datums, true) {
        Ok(v) => (v, None),
        Err(e) => (String::new(), Some(e)),
    };
    terror::Log(err.map(errors::Trace));
    str_value
}

// CloneRow deep copies a Datum slice.
// 深拷贝一行 Datum。
pub fn CloneRow(dr: &[Datum]) -> Vec<Datum> {
    let mut c = vec![Datum::default(); dr.len()];
    for (i, d) in dr.iter().enumerate() {
        d.Copy(&mut c[i]);
    }
    c
}

// GetMaxValue returns the max value datum for each type.
// 按 FieldType 取类型上界 Datum。
pub fn GetMaxValue(ft: &FieldType) -> Datum {
    let mut maxVal = Datum::default();
    match ft.GetType() {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong => {
            if mysql::HasUnsignedFlag(ft.GetFlag()) {
                maxVal.SetUint64(IntegerUnsignedUpperBound(ft.GetType()));
            } else {
                maxVal.SetInt64(IntegerSignedUpperBound(ft.GetType()));
            }
        }
        mysql::TypeFloat => {
            maxVal.SetFloat32(GetMaxFloat(ft.GetFlen() as i32, ft.GetDecimal() as i32) as f32)
        }
        mysql::TypeDouble => {
            maxVal.SetFloat64(GetMaxFloat(ft.GetFlen() as i32, ft.GetDecimal() as i32))
        }
        mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeVarchar
        | mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob => maxVal.SetString(
            hack::String(&vec![250]).to_string(),
            ft.GetCollate().to_owned(),
        ),
        mysql::TypeNewDecimal => maxVal.SetMysqlDecimal(NewMaxOrMinDec(
            false,
            ft.GetFlen() as usize,
            ft.GetDecimal() as usize,
        )),
        mysql::TypeDuration => maxVal.SetMysqlDuration(Duration {
            Duration: MaxTime,
            Fsp: 0,
        }),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
            if ft.GetType() == mysql::TypeDate || ft.GetType() == mysql::TypeDatetime {
                let mut value = MaxDatetime();
                value.SetType(ft.GetType());
                maxVal.SetMysqlTime(value);
            } else {
                maxVal.SetMysqlTime(MaxTimestamp());
            }
        }
        _ => {}
    }
    maxVal
}

// GetMinValue returns the min value datum for each type.
// 按 FieldType 取类型下界 Datum。
pub fn GetMinValue(ft: &FieldType) -> Datum {
    let mut minVal = Datum::default();
    match ft.GetType() {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong => {
            if mysql::HasUnsignedFlag(ft.GetFlag()) {
                minVal.SetUint64(0);
            } else {
                minVal.SetInt64(IntegerSignedLowerBound(ft.GetType()));
            }
        }
        mysql::TypeFloat => {
            minVal.SetFloat32(-GetMaxFloat(ft.GetFlen() as i32, ft.GetDecimal() as i32) as f32)
        }
        mysql::TypeDouble => {
            minVal.SetFloat64(-GetMaxFloat(ft.GetFlen() as i32, ft.GetDecimal() as i32))
        }
        mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeVarchar
        | mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob => minVal.SetString(
            hack::String(&vec![1]).to_string(),
            ft.GetCollate().to_owned(),
        ),
        mysql::TypeNewDecimal => minVal.SetMysqlDecimal(NewMaxOrMinDec(
            true,
            ft.GetFlen() as usize,
            ft.GetDecimal() as usize,
        )),
        mysql::TypeDuration => minVal.SetMysqlDuration(Duration {
            Duration: MinTime,
            Fsp: 0,
        }),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp => {
            if ft.GetType() == mysql::TypeDate || ft.GetType() == mysql::TypeDatetime {
                let mut value = MinDatetime();
                value.SetType(ft.GetType());
                minVal.SetMysqlTime(value);
            } else {
                minVal.SetMysqlTime(MinTimestamp());
            }
        }
        _ => {}
    }
    minVal
}

// RoundingType is used to indicate the rounding type for reversing evaluation.
// 取整方向类型别名。
pub type RoundingType = u8;

// Ceiling means rounding up.
// 向 +∞ 取整。
pub const Ceiling: RoundingType = 0;
// Floor means rounding down.
// 向 -∞ 取整。
pub const Floor: RoundingType = 1;

// getDatumBound 根据 rounding 类型选择某个 FieldType 的最大或最小 Datum。
pub fn getDatumBound(retType: &FieldType, rType: RoundingType) -> Datum {
    if rType == Ceiling {
        return GetMaxValue(retType);
    }
    GetMinValue(retType)
}

// ChangeReverseResultByUpperLowerBound is for expression's reverse evaluation.
// 反转结果后按类型上下界修正。
pub fn ChangeReverseResultByUpperLowerBound(
    ctx: Context,
    retType: &FieldType,
    res: Datum,
    rType: RoundingType,
) -> Result<Datum, errors::Error> {
    let mut d = match res.ConvertTo(ctx.clone(), retType) {
        Ok(v) => v,
        Err(err) if terror::ErrorEqual(&err, &*ErrOverflow) => return Ok(Datum::default()),
        Err(err) => return Err(err),
    };

    let mut resRetType = FieldType::default();
    match res.Kind() {
        KindInt64 => resRetType.SetType(mysql::TypeLonglong),
        KindUint64 => {
            resRetType.SetType(mysql::TypeLonglong);
            resRetType.AddFlag(mysql::UnsignedFlag);
        }
        KindFloat32 => resRetType.SetType(mysql::TypeFloat),
        KindFloat64 => resRetType.SetType(mysql::TypeDouble),
        KindMysqlDecimal => {
            resRetType.SetType(mysql::TypeNewDecimal);
            resRetType.SetFlenUnderLimit(
                (res.GetMysqlDecimal().GetDigitsFrac() + res.GetMysqlDecimal().GetDigitsInt())
                    as isize,
            );
            resRetType.SetDecimalUnderLimit(res.GetMysqlDecimal().GetDigitsInt() as isize);
        }
        _ => {}
    }

    let bound = getDatumBound(&resRetType, rType);
    let collator = collate::GetCollator(resRetType.GetCollate());
    let cmp = d.Compare(ctx.clone(), &bound, collator.as_ref())?;
    if cmp == 0 {
        d = getDatumBound(retType, rType);
    } else if rType == Ceiling {
        // Go 只在 ceiling 时把整数/浮点/decimal 的反推结果向上推进一个单位。
        match retType.GetType() {
            mysql::TypeShort => bump_integer_bound(
                &mut d,
                retType,
                math::MaxUint16 as u64,
                math::MaxInt16 as i64,
            ),
            mysql::TypeLong => bump_integer_bound(
                &mut d,
                retType,
                math::MaxUint32 as u64,
                math::MaxInt32 as i64,
            ),
            mysql::TypeLonglong => {
                bump_integer_bound(&mut d, retType, math::MaxUint64, math::MaxInt64)
            }
            mysql::TypeFloat => {
                if d.GetFloat32() != math::MaxFloat32 as f32 {
                    d.SetFloat32(d.GetFloat32() + 1.0);
                }
            }
            mysql::TypeDouble => {
                if d.GetFloat64() != math::MaxFloat64 {
                    d.SetFloat64(d.GetFloat64() + 1.0);
                }
            }
            mysql::TypeNewDecimal => {
                if d.GetMysqlDecimal().Compare(&NewMaxOrMinDec(
                    false,
                    retType.GetFlen() as usize,
                    retType.GetDecimal() as usize,
                )) != 0
                {
                    let mut one = MyDecimal::default();
                    one.FromInt(1);
                    let mut newD = MyDecimal::default();
                    DecimalAdd(&d.GetMysqlDecimal(), &one, &mut newD)?;
                    d = NewDecimalDatum(newD);
                }
            }
            _ => {}
        }
    }
    Ok(d)
}

/// 空 Datum 大小常量。
pub const sizeOfEmptyDatum: i32 = 72;
/// Time 大小常量。
pub const sizeOfMysqlTime: i32 = 16;
/// MyDecimal 大小常量。
pub const sizeOfMyDecimal: i32 = MyDecimalStructSize as i32;

// EstimatedMemUsage returns the estimated bytes consumed of a one-dimensional or two-dimensional datum array.
// 估算多行 Datum 内存占用。
pub fn EstimatedMemUsage(array: &[Datum], numOfRows: i32) -> i64 {
    if numOfRows == 0 {
        return 0;
    }
    let mut bytesConsumed = 0_i64;
    for d in array {
        bytesConsumed += d.EstimatedMemUsage();
    }
    bytesConsumed * numOfRows as i64
}

// DatumsContainNull return true if any value is null.
// 切片中是否含 NULL。
pub fn DatumsContainNull(vals: &[Datum]) -> bool {
    vals.iter().any(|val| val.IsNull())
}

// utf8_safe_prefix_len 对应 ProduceStrWithSpecifiedTp 中为 blob/text 查找完整 rune 截断点。
fn utf8_safe_prefix_len(s: &str, flen: usize) -> usize {
    let mut truncateLen = flen.min(s.len());
    while truncateLen > 0 && !s.is_char_boundary(truncateLen) {
        truncateLen -= 1;
    }
    truncateLen
}

// nth_rune_boundary 对应 Go range 遍历字符串找到第 flen 个字符的字节位置。
fn nth_rune_boundary(s: &str, flen: usize) -> usize {
    if flen == 0 {
        return 0;
    }
    s.char_indices()
        .nth(flen)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| s.len())
}

// cmp_to_ordering 把 Go cmp 的 -1/0/1 转成 Rust sort 需要的 Ordering。
fn cmp_to_ordering(cmp: i32) -> std::cmp::Ordering {
    if cmp < 0 {
        std::cmp::Ordering::Less
    } else if cmp > 0 {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

// bump_integer_bound 保留 ChangeReverseResultByUpperLowerBound 中三种整数类型的加一逻辑。
fn bump_integer_bound(d: &mut Datum, retType: &FieldType, max_u: u64, max_i: i64) {
    if mysql::HasUnsignedFlag(retType.GetFlag()) {
        if d.GetUint64() != max_u {
            d.SetUint64(d.GetUint64() + 1);
        }
    } else if d.GetInt64() != max_i {
        d.SetInt64(d.GetInt64() + 1);
    }
}
