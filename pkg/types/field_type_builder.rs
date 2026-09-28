// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// FieldType 的链式建造器（Builder）。
//
// 封装对类型、flag、flen、decimal、charset/collation、elems 等字段的
// 逐步设置，最后 `Build` 产出 `FieldType` 值，`BuildP` 借用内部字段。

/// FieldType 链式建造器，内部持有待构建的 FieldType。
pub struct FieldTypeBuilder {
    ft: FieldType,
}

/// 创建默认空 FieldType 的建造器。
pub fn NewFieldTypeBuilder() -> Box<FieldTypeBuilder> {
    Box::new(FieldTypeBuilder {
        ft: FieldType::default(),
    })
}

impl FieldTypeBuilder {
    /// 读取当前类型码。
    pub fn GetType(&self) -> u8 {
        self.ft.GetType()
    }
    /// 读取当前 flag 位图。
    pub fn GetFlag(&self) -> usize {
        self.ft.GetFlag()
    }
    /// 读取显示宽度/长度 flen。
    pub fn GetFlen(&self) -> isize {
        self.ft.GetFlen()
    }
    /// 读取小数位/精度 decimal。
    pub fn GetDecimal(&self) -> isize {
        self.ft.GetDecimal()
    }
    /// 读取字符集。
    pub fn GetCharset(&self) -> &str {
        self.ft.GetCharset()
    }
    /// 读取校对规则（collation）。
    pub fn GetCollate(&self) -> &str {
        self.ft.GetCollate()
    }

    /// 设置类型码并返回 self 以支持链式调用。
    pub fn SetType(&mut self, tp: u8) -> &mut Self {
        self.ft.SetType(tp);
        self
    }
    /// 覆盖设置 flag 位图。
    pub fn SetFlag(&mut self, flag: usize) -> &mut Self {
        self.ft.SetFlag(flag);
        self
    }
    /// 按位或追加 flag。
    pub fn AddFlag(&mut self, flag: usize) -> &mut Self {
        self.ft.AddFlag(flag);
        self
    }
    /// 按位异或翻转 flag。
    pub fn ToggleFlag(&mut self, flag: usize) -> &mut Self {
        self.ft.ToggleFlag(flag);
        self
    }
    /// 清除指定 flag 位。
    pub fn DelFlag(&mut self, flag: usize) -> &mut Self {
        self.ft.DelFlag(flag);
        self
    }
    /// 设置 flen。
    pub fn SetFlen(&mut self, flen: isize) -> &mut Self {
        self.ft.SetFlen(flen);
        self
    }
    /// 设置 decimal。
    pub fn SetDecimal(&mut self, decimal: isize) -> &mut Self {
        self.ft.SetDecimal(decimal);
        self
    }
    /// 设置字符集。
    pub fn SetCharset(&mut self, charset: String) -> &mut Self {
        self.ft.SetCharset(charset);
        self
    }
    /// 设置 collation。
    pub fn SetCollate(&mut self, collate: String) -> &mut Self {
        self.ft.SetCollate(collate);
        self
    }
    /// 设置 ENUM/SET 元素列表。
    pub fn SetElems(&mut self, elems: Vec<String>) -> &mut Self {
        self.ft.SetElems(elems);
        self
    }
    /// 设置是否为数组类型标记。
    pub fn SetArray(&mut self, value: bool) -> &mut Self {
        self.ft.SetArray(value);
        self
    }
    /// 克隆产出 FieldType 值。
    pub fn Build(&self) -> FieldType {
        self.ft.clone()
    }
    /// 返回内部 FieldType 的可变借用，对齐 Go `&b.ft` 的别名语义。
    pub fn BuildP(&mut self) -> &mut FieldType {
        &mut self.ft
    }
}
