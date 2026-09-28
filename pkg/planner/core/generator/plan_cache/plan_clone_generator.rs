// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 物理计划 `CloneForPlanCache` 方法的代码生成器。
//
// 计划缓存命中后需克隆物理计划树并换上新会话的 PlanContext。
// 字段按 tag 分为 Deep（深拷贝）、Shallow（共享）与 MustNil（非空则拒绝缓存）。
// 特殊字段（如 TablePlans 展平）保留与 Go 生成器相同的硬编码分支。

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// Go 生成器覆盖的全部物理算子类型名（顺序固定）。
pub const PHYSICAL_STRUCTURES: &[&str] = &[
    "Update",
    "Delete",
    "Insert",
    "PhysicalTableScan",
    "PhysicalIndexScan",
    "PhysicalSelection",
    "PhysicalProjection",
    "PhysicalTopN",
    "PhysicalLimit",
    "PhysicalStreamAgg",
    "PhysicalHashAgg",
    "PhysicalHashJoin",
    "PhysicalMergeJoin",
    "PhysicalIndexJoin",
    "PhysicalIndexHashJoin",
    "PhysicalIndexReader",
    "PhysicalTableReader",
    "PhysicalIndexMergeReader",
    "PhysicalIndexLookUpReader",
    "PhysicalLocalIndexLookUp",
    "BatchPointGetPlan",
    "PointGetPlan",
    "PhysicalUnionScan",
    "PhysicalUnionAll",
    "PhysicalTableDual",
];

#[derive(Clone, Debug, Eq, PartialEq)]
/// 生成失败错误（信息字符串包装）。
pub struct Error(String);

impl Error {
    /// 由任意可转 String 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
/// 本模块统一的 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 字段克隆策略标签（对应 Go struct tag）。
pub enum CloneTag {
    #[default]
    /// 深拷贝（默认）。
    Deep,
    /// 浅共享，可直接沿用原指针/标量。
    Shallow,
    /// 缓存路径上必须为 nil，否则拒绝克隆。
    MustNil,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Go 基础类型种类，用于判断可否浅克隆。
pub enum FieldKind {
    Bool,
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Float32,
    Float64,
    String,
    Composite,
}

impl FieldKind {
    /// 是否为标量（非 Composite）。
    pub fn is_scalar(self) -> bool {
        self != Self::Composite
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 结构体字段元数据：名、Go 类型串、种类与克隆标签。
pub struct StructField {
    /// 字段名。
    /// 结构体类型名。
    pub name: String,
    /// Go 类型字符串（如 `[]expression.Expression`）。
    pub field_type: String,
    /// 推导出的字段种类。
    pub kind: FieldKind,
    /// 克隆策略。
    pub tag: CloneTag,
}

impl StructField {
    /// 由字段名与类型串构造，默认 Deep，并推导 scalar kind。
    pub fn new(name: impl Into<String>, field_type: impl Into<String>) -> Self {
        let field_type = field_type.into();
        Self {
            name: name.into(),
            kind: scalar_kind(&field_type),
            field_type,
            tag: CloneTag::Deep,
        }
    }

    /// 覆盖克隆标签后返回自身（建造者模式）。
    pub fn with_tag(mut self, tag: CloneTag) -> Self {
        self.tag = tag;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 待生成 CloneForPlanCache 的结构体描述。
pub struct Structure {
    /// Go 包名（通常为 physicalop）。
    pub package: String,
    pub name: String,
    /// 字段列表。
    pub fields: Vec<StructField>,
}

impl Structure {
    /// 构造 physicalop 包下的物理算子结构描述。
    pub fn physical(name: impl Into<String>, fields: Vec<StructField>) -> Self {
        Self {
            package: "physicalop".to_owned(),
            name: name.into(),
            fields,
        }
    }

    /// 返回 `package.name` 全名，用于特殊字段匹配。
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.package, self.name)
    }
}

/// 将 Go 类型名映射为 FieldKind；未知则视为 Composite。
fn scalar_kind(field_type: &str) -> FieldKind {
    match field_type {
        "bool" => FieldKind::Bool,
        "int" => FieldKind::Int,
        "int8" => FieldKind::Int8,
        "int16" => FieldKind::Int16,
        "int32" => FieldKind::Int32,
        "int64" => FieldKind::Int64,
        "uint" => FieldKind::Uint,
        "uint8" | "byte" => FieldKind::Uint8,
        "uint16" => FieldKind::Uint16,
        "uint32" => FieldKind::Uint32,
        "uint64" => FieldKind::Uint64,
        "float32" => FieldKind::Float32,
        "float64" | "float" => FieldKind::Float64,
        "string" => FieldKind::String,
        _ => FieldKind::Composite,
    }
}

/// 字段是否标记为 MustNil。
pub fn must_nil_field(field: &StructField) -> bool {
    field.tag == CloneTag::MustNil
}

/// 是否允许浅克隆（Shallow 标签或标量种类）。
pub fn allow_shallow_clone(field: &StructField) -> bool {
    field.tag == CloneTag::Shallow || field.kind.is_scalar()
}

#[derive(Default)]
/// 简易 Go 源码缓冲：维护缩进并按行写出。
struct CodeGen {
    buffer: String,
    indent: usize,
}

impl CodeGen {
    /// 原样追加文本，确保以换行结尾。
    fn raw(&mut self, text: &str) {
        self.buffer.push_str(text);
        if !text.ends_with('\n') {
            self.buffer.push('\n');
        }
    }

    /// 按当前缩进写一行；遇 `{`/`}` 调整缩进层级。
    fn line(&mut self, line: impl AsRef<str>) {
        let line = line.as_ref();
        if line.starts_with('}') {
            self.indent = self.indent.saturating_sub(1);
        }
        for _ in 0..self.indent {
            self.buffer.push('\t');
        }
        self.buffer.push_str(line);
        self.buffer.push('\n');
        if line.ends_with('{') {
            self.indent += 1;
        }
    }

    /// 结束生成：缩进未归零则报错，否则返回字节。
    fn finish(self) -> Result<Vec<u8>> {
        if self.indent != 0 {
            return Err(Error::new("generated Go source has unbalanced blocks"));
        }
        Ok(self.buffer.into_bytes())
    }
}

/// 为单个结构体生成 `CloneForPlanCache` 方法源码。
pub fn gen_plan_clone_for_plan_cache(structure: &Structure) -> Result<Vec<u8>> {
    let mut generator = CodeGen::default();
    generator.line("// CloneForPlanCache implements the base.Plan interface.");
    generator.line(format!(
        "func (op *{}) CloneForPlanCache(newCtx base.PlanContext) (base.Plan, bool) {{",
        structure.name
    ));
    generator.line(format!("cloned := new({})", structure.name));
    generator.line("*cloned = *op");
    // 跳过可浅克隆字段；MustNil 非空则失败；其余走特殊或通用深拷贝。
    for field in &structure.fields {
        if allow_shallow_clone(field) {
            continue;
        }
        if must_nil_field(field) {
            generator.line(format!("if op.{} != nil {{", field.name));
            generator.line("return nil, false");
            generator.line("}");
            continue;
        }
        if write_special_field(&mut generator, structure, field) {
            continue;
        }
        write_field_clone(&mut generator, structure, field)?;
    }
    generator.line("return cloned, true");
    generator.line("}");
    generator.finish()
}

/// 处理需硬编码逻辑的特殊字段（如 Reader 的 Plans 展平）。
fn write_special_field(
    generator: &mut CodeGen,
    structure: &Structure,
    field: &StructField,
) -> bool {
    let full_name = format!("{}.{}", structure.full_name(), field.name);
    match full_name.as_str() {
        "physicalop.PhysicalTableReader.TablePlans"
        | "physicalop.PhysicalIndexLookUpReader.TablePlans"
        | "physicalop.PhysicalIndexMergeReader.TablePlans" => {
            generator.line("cloned.TablePlans = FlattenListPushDownPlan(cloned.TablePlan)");
        }
        "physicalop.PhysicalIndexReader.IndexPlans" => {
            generator.line("cloned.IndexPlans = FlattenListPushDownPlan(cloned.IndexPlan)");
        }
        "physicalop.PhysicalIndexLookUpReader.IndexPlans" => {
            generator.line("if cloned.IndexLookUpPushDown {");
            generator.line("cloned.IndexPlans, cloned.IndexPlansUnNatureOrders = FlattenTreePushDownPlan(cloned.IndexPlan)");
            generator.line("} else {");
            generator.line("cloned.IndexPlans = FlattenListPushDownPlan(cloned.IndexPlan)");
            generator.line("}");
        }
        "physicalop.PhysicalIndexMergeReader.PartialPlans" => {
            generator
                .line("cloned.PartialPlans = make([][]base.PhysicalPlan, len(op.PartialPlans))");
            generator.line("for i, plan := range cloned.PartialPlansRaw {");
            generator.line("cloned.PartialPlans[i] = FlattenListPushDownPlan(plan)");
            generator.line("}");
        }
        _ => return false,
    }
    true
}

/// 按字段 Go 类型写出深拷贝/守卫克隆语句。
fn write_field_clone(
    generator: &mut CodeGen,
    structure: &Structure,
    field: &StructField,
) -> Result<()> {
    let name = &field.name;
    let field_type = field.field_type.as_str();
    match field_type {
        "[]int" | "[]byte" | "[]float" | "[]bool" | "[]uint32" => {
            generator.line(format!(
                "cloned.{name} = make({field_type}, len(op.{name}))"
            ));
            generator.line(format!("copy(cloned.{name}, op.{name})"));
        }
        "physicalop.BasePhysicalAgg"
        | "physicalop.BasePhysicalJoin"
        | "physicalop.BasePhysicalPlan"
        | "physicalop.PhysicalSchemaProducer" => {
            let embedded = field_type.split('.').next_back().unwrap_or(field_type);
            generator.line(format!(
                "basePlan, baseOK := op.{embedded}.CloneForPlanCacheWithSelf(newCtx, cloned)"
            ));
            write_abort(generator, "baseOK");
            generator.line(format!("cloned.{embedded} = *basePlan"));
        }
        "baseimpl.Plan" => generator.line(format!(
            "cloned.{name} = *op.{name}.CloneWithNewCtx(newCtx)"
        )),
        "physicalop.SimpleSchemaProducer" => generator.line(format!(
            "cloned.{name} = *op.{name}.CloneSelfForPlanCache(newCtx)"
        )),
        "[]expression.Expression"
        | "[]*expression.Column"
        | "[]*expression.Constant"
        | "[]*expression.ScalarFunction" => {
            let structure_name = format!(
                "{}s",
                field_type.split('.').next_back().unwrap_or(field_type)
            );
            generator.line(format!(
                "cloned.{name} = utilfuncp.Clone{structure_name}ForPlanCache(op.{name}, nil)"
            ));
        }
        "[][]expression.Expression" => generator.line(format!(
            "cloned.{name} = utilfuncp.CloneExpression2DForPlanCache(op.{name})"
        )),
        "[][]*expression.Constant" => generator.line(format!(
            "cloned.{name} = CloneConstant2DForPlanCache(op.{name})"
        )),
        "[]*ranger.Range" | "[]*util.ByItems" | "[]property.SortItem" => {
            generator.line(format!("cloned.{name} = sliceutil.DeepClone(op.{name})"))
        }
        "[]model.CIStr" | "[]types.Datum" | "[]kv.Handle" | "[]*expression.Assignment" => {
            let structure_name = format!(
                "{}s",
                field_type.split('.').next_back().unwrap_or(field_type)
            );
            generator.line(format!(
                "cloned.{name} = util.Clone{structure_name}(op.{name})"
            ));
        }
        "[][]types.Datum" => {
            generator.line(format!("cloned.{name} = util.CloneDatum2D(op.{name})"));
        }
        "[]*types.FieldName" => {
            generator.line(format!("cloned.{name} = util.CloneFieldNames(op.{name})"))
        }
        "planctx.PlanContext" => generator.line(format!("cloned.{name} = newCtx")),
        "util.HandleCols" => write_guarded(generator, name, "Clone()"),
        "*physicalop.PushedDownLimit" => {
            generator.line(format!("cloned.{name} = op.{name}.Clone()"));
        }
        "*physicalop.PhysPlanPartInfo" => {
            generator.line(format!("cloned.{name} = op.{name}.CloneForPlanCache()"))
        }
        "*physicalop.ColWithCmpFuncManager" | "physicalop.InsertGeneratedColumns" => {
            generator.line(format!("cloned.{name} = op.{name}.cloneForPlanCache()"))
        }
        "kv.Handle" => write_guarded(generator, name, "Copy()"),
        "*expression.Column" | "*expression.Constant" => {
            generator.line(format!("if op.{name} != nil {{"));
            generator.line(format!("if op.{name}.SafeToShareAcrossSession() {{"));
            generator.line(format!("cloned.{name} = op.{name}"));
            generator.line("} else {");
            generator.line(format!("cloned.{name} = op.{name}.Clone().({field_type})"));
            generator.line("}");
            generator.line("}");
        }
        "physicalop.PhysicalIndexJoin" => {
            generator.line(format!("inlj, ok := op.{name}.CloneForPlanCache(newCtx)"));
            write_abort(generator, "ok");
            generator.line(format!("cloned.{name} = *inlj.(*PhysicalIndexJoin)"));
            generator.line("cloned.Self = cloned");
        }
        "base.PhysicalPlan" => {
            generator.line(format!("if op.{name} != nil {{"));
            generator.line(format!("{name}, ok := op.{name}.CloneForPlanCache(newCtx)"));
            write_abort(generator, "ok");
            generator.line(format!("cloned.{name} = {name}.(base.PhysicalPlan)"));
            generator.line("}");
        }
        "[]base.PhysicalPlan" => {
            generator.line(format!(
                "{name}, ok := ClonePhysicalPlansForPlanCache(newCtx, op.{name})"
            ));
            write_abort(generator, "ok");
            generator.line(format!("cloned.{name} = {name}"));
        }
        "*int" => {
            generator.line(format!("if op.{name} != nil {{"));
            generator.line(format!("cloned.{name} = new(int)"));
            generator.line(format!("*cloned.{name} = *op.{name}"));
            generator.line("}");
        }
        "ranger.MutableRanges" => {
            generator.line(format!("cloned.{name} = op.{name}.CloneForPlanCache()"))
        }
        "map[int64][]util.HandleCols" => {
            write_map_clone(generator, name, field_type, "util.CloneHandleCols(v)")
        }
        "map[int64]*expression.Column" => write_map_clone(
            generator,
            name,
            field_type,
            "v.Clone().(*expression.Column)",
        ),
        "map[int]int" => write_map_clone(generator, name, field_type, "v"),
        _ => {
            return Err(Error::new(format!(
                "can't generate Clone method for type {field_type} in {}",
                structure.full_name()
            )));
        }
    }
    Ok(())
}

fn write_abort(generator: &mut CodeGen, condition: &str) {
    generator.line(format!("if !{condition} {{"));
    generator.line("return nil, false");
    generator.line("}");
}

/// 非空时调用 `op.field.call` 赋给 cloned。
fn write_guarded(generator: &mut CodeGen, name: &str, call: &str) {
    generator.line(format!("if op.{name} != nil {{"));
    generator.line(format!("cloned.{name} = op.{name}.{call}"));
    generator.line("}");
}

/// 非空时 make+range 拷贝 map，值表达式由调用方给定。
fn write_map_clone(generator: &mut CodeGen, name: &str, map_type: &str, value: &str) {
    generator.line(format!("if op.{name} != nil {{"));
    generator.line(format!("cloned.{name} = make({map_type}, len(op.{name}))"));
    generator.line(format!("for k, v := range op.{name} {{"));
    generator.line(format!("cloned.{name}[k] = {value}"));
    generator.line("}");
    generator.line("}");
}

/// 拼接版权头与多个结构体的 CloneForPlanCache 方法。
pub fn generate_plan_clone_for_plan_cache_code(structures: &[Structure]) -> Result<Vec<u8>> {
    let mut generator = CodeGen::default();
    generator.raw(CODE_GEN_PLAN_CACHE_PREFIX);
    for (index, structure) in structures.iter().enumerate() {
        let code = gen_plan_clone_for_plan_cache(structure)?;
        generator.raw(std::str::from_utf8(&code).map_err(|error| Error::new(error.to_string()))?);
        if index + 1 < structures.len() {
            generator.raw("\n");
        }
    }
    generator.finish()
}

/// Rust 无法在运行时反射任意结构体字段；调用方需提供 Go reflect 同级元数据。
/// 本目录保留物理计划类型的完整集合与顺序。
/// Rust cannot reflect arbitrary struct fields at runtime. Callers provide the
/// same metadata that Go obtains through reflect; this catalog preserves the
/// complete set and ordering of physical plan types.
pub fn physical_structure_catalog() -> BTreeMap<String, Structure> {
    PHYSICAL_STRUCTURES
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                Structure::physical(*name, physical_structure_fields(name)),
            )
        })
        .collect()
}

/// Field metadata mirrored from the Go `reflect.Type` input to the generator.
///
/// Rust cannot reflect the Go physical plan structs, so this table is kept in the
/// same order as the generated Go methods. Scalar fields are intentionally omitted:
/// `allowShallowClone` skips them in both implementations.
fn physical_structure_fields(name: &str) -> Vec<StructField> {
    let field = |name: &str, field_type: &str| StructField::new(name, field_type);
    match name {
        "Update" => vec![
            field("SimpleSchemaProducer", "physicalop.SimpleSchemaProducer"),
            field("OrderedList", "[]*expression.Assignment"),
            field("SelectPlan", "base.PhysicalPlan"),
            field("FKChecks", "[]any").with_tag(CloneTag::MustNil),
            field("FKCascades", "[]any").with_tag(CloneTag::MustNil),
        ],
        "Delete" => vec![
            field("SimpleSchemaProducer", "physicalop.SimpleSchemaProducer"),
            field("SelectPlan", "base.PhysicalPlan"),
            field("FKChecks", "[]any").with_tag(CloneTag::MustNil),
            field("FKCascades", "[]any").with_tag(CloneTag::MustNil),
        ],
        "Insert" => vec![
            field("SimpleSchemaProducer", "physicalop.SimpleSchemaProducer"),
            field("Lists", "[][]expression.Expression"),
            field("OnDuplicate", "[]*expression.Assignment"),
            field("GenCols", "physicalop.InsertGeneratedColumns"),
            field("SelectPlan", "base.PhysicalPlan"),
            field("FKChecks", "[]any").with_tag(CloneTag::MustNil),
            field("FKCascades", "[]any").with_tag(CloneTag::MustNil),
        ],
        "PhysicalTableScan" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("AccessCondition", "[]expression.Expression"),
            field("FilterCondition", "[]expression.Expression"),
            field(
                "LateMaterializationFilterCondition",
                "[]expression.Expression",
            ),
            field("HandleIdx", "[]int"),
            field("HandleCols", "util.HandleCols"),
            field("ByItems", "[]*util.ByItems"),
            field("PlanPartInfo", "*physicalop.PhysPlanPartInfo"),
            field("SampleInfo", "[]any").with_tag(CloneTag::MustNil),
            field("constColsByCond", "[]bool"),
            field("runtimeFilterList", "[]any").with_tag(CloneTag::MustNil),
            field("UsedColumnarIndexes", "[]any").with_tag(CloneTag::MustNil),
        ],
        "PhysicalIndexScan" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("AccessCondition", "[]expression.Expression"),
            field("IdxCols", "[]*expression.Column"),
            field("IdxColLens", "[]int"),
            field("GenExprs", "[]any").with_tag(CloneTag::MustNil),
            field("ByItems", "[]*util.ByItems"),
            field("PkIsHandleCol", "*expression.Column"),
            field("ConstColsByCond", "[]bool"),
        ],
        "PhysicalSelection" => vec![
            field("BasePhysicalPlan", "physicalop.BasePhysicalPlan"),
            field("Conditions", "[]expression.Expression"),
        ],
        "PhysicalProjection" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("Exprs", "[]expression.Expression"),
        ],
        "PhysicalTopN" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("ByItems", "[]*util.ByItems"),
            field("PartitionBy", "[]property.SortItem"),
            field("PrefixCol", "*expression.Column"),
        ],
        "PhysicalLimit" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("PartitionBy", "[]property.SortItem"),
            field("PrefixCol", "*expression.Column"),
        ],
        "PhysicalStreamAgg" | "PhysicalHashAgg" => {
            vec![field("BasePhysicalAgg", "physicalop.BasePhysicalAgg")]
        }
        "PhysicalHashJoin" => vec![
            field("BasePhysicalJoin", "physicalop.BasePhysicalJoin"),
            field("EqualConditions", "[]*expression.ScalarFunction"),
            field("NAEqualConditions", "[]*expression.ScalarFunction"),
            field("runtimeFilterList", "[]any").with_tag(CloneTag::MustNil),
        ],
        "PhysicalMergeJoin" => vec![field("BasePhysicalJoin", "physicalop.BasePhysicalJoin")],
        "PhysicalIndexJoin" => vec![
            field("BasePhysicalJoin", "physicalop.BasePhysicalJoin"),
            field("InnerPlan", "base.PhysicalPlan"),
            field("Ranges", "ranger.MutableRanges"),
            field("KeyOff2IdxOff", "[]int"),
            field("IdxColLens", "[]int"),
            field("CompareFilters", "*physicalop.ColWithCmpFuncManager"),
            field("OuterHashKeys", "[]*expression.Column"),
            field("InnerHashKeys", "[]*expression.Column"),
        ],
        "PhysicalIndexHashJoin" => vec![field("PhysicalIndexJoin", "physicalop.PhysicalIndexJoin")],
        "PhysicalIndexReader" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("IndexPlan", "base.PhysicalPlan"),
            field("IndexPlans", "[]base.PhysicalPlan"),
            field("OutputColumns", "[]*expression.Column"),
            field("PlanPartInfo", "*physicalop.PhysPlanPartInfo"),
        ],
        "PhysicalTableReader" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("TablePlan", "base.PhysicalPlan"),
            field("TablePlans", "[]base.PhysicalPlan"),
            field("PlanPartInfo", "*physicalop.PhysPlanPartInfo"),
            field("TableScanAndPartitionInfos", "[]any").with_tag(CloneTag::MustNil),
        ],
        "PhysicalIndexMergeReader" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("PushedLimit", "*physicalop.PushedDownLimit"),
            field("ByItems", "[]*util.ByItems"),
            field("PartialPlansRaw", "[]base.PhysicalPlan"),
            field("TablePlan", "base.PhysicalPlan"),
            field("PartialPlans", "[][]base.PhysicalPlan"),
            field("TablePlans", "[]base.PhysicalPlan"),
            field("PlanPartInfo", "*physicalop.PhysPlanPartInfo"),
            field("HandleCols", "util.HandleCols"),
        ],
        "PhysicalIndexLookUpReader" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("IndexPlan", "base.PhysicalPlan"),
            field("TablePlan", "base.PhysicalPlan"),
            field("IndexPlans", "[]base.PhysicalPlan"),
            field("IndexPlansUnNatureOrders", "map[int]int"),
            field("TablePlans", "[]base.PhysicalPlan"),
            field("ExtraHandleCol", "*expression.Column"),
            field("PushedLimit", "*physicalop.PushedDownLimit"),
            field("CommonHandleCols", "[]*expression.Column"),
            field("PlanPartInfo", "*physicalop.PhysPlanPartInfo"),
        ],
        "PhysicalLocalIndexLookUp" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("IndexHandleOffsets", "[]uint32"),
        ],
        "BatchPointGetPlan" => vec![
            field("SimpleSchemaProducer", "physicalop.SimpleSchemaProducer"),
            field("ProbeParents", "[]base.PhysicalPlan"),
            field("ctx", "planctx.PlanContext"),
            field("Handles", "[]kv.Handle"),
            field("HandleParams", "[]*expression.Constant"),
            field("IndexValues", "[][]types.Datum"),
            field("IndexValueParams", "[][]*expression.Constant"),
            field("AccessConditions", "[]expression.Expression"),
            field("IdxCols", "[]*expression.Column"),
            field("IdxColLens", "[]int"),
            field("PartitionIdxs", "[]int"),
            field("accessCols", "[]*expression.Column"),
        ],
        "PointGetPlan" => vec![
            field("Plan", "baseimpl.Plan"),
            field("PartitionIdx", "*int"),
            field("Handle", "kv.Handle"),
            field("HandleConstant", "*expression.Constant"),
            field("IndexValues", "[]types.Datum"),
            field("IndexConstants", "[]*expression.Constant"),
            field("IdxCols", "[]*expression.Column"),
            field("IdxColLens", "[]int"),
            field("AccessConditions", "[]expression.Expression"),
            field("accessCols", "[]*expression.Column"),
        ],
        "PhysicalUnionScan" => vec![
            field("BasePhysicalPlan", "physicalop.BasePhysicalPlan"),
            field("Conditions", "[]expression.Expression"),
            field("HandleCols", "util.HandleCols"),
        ],
        "PhysicalUnionAll" => vec![field(
            "PhysicalSchemaProducer",
            "physicalop.PhysicalSchemaProducer",
        )],
        "PhysicalTableDual" => vec![
            field(
                "PhysicalSchemaProducer",
                "physicalop.PhysicalSchemaProducer",
            ),
            field("names", "[]*types.FieldName"),
        ],
        _ => Vec::new(),
    }
}

#[allow(non_snake_case)]
/// 对应 Go 入口：按 PHYSICAL_STRUCTURES 顺序生成全部克隆代码。
pub fn GenPlanCloneForPlanCacheCode() -> Result<Vec<u8>> {
    let catalog = physical_structure_catalog();
    let structures = PHYSICAL_STRUCTURES
        .iter()
        .map(|name| catalog.get(*name).cloned().unwrap())
        .collect::<Vec<_>>();
    generate_plan_clone_for_plan_cache_code(&structures)
}

/// 生成并写入目标文件（覆盖写）。
pub fn write_generated_file(path: impl AsRef<Path>, structures: &[Structure]) -> Result<()> {
    let data = generate_plan_clone_for_plan_cache_code(structures)?;
    std::fs::write(path, data).map_err(|error| Error::new(error.to_string()))
}

// 生成文件固定前缀：版权、package physicalop 与必要 import。
const CODE_GEN_PLAN_CACHE_PREFIX: &str = r#"// Copyright 2024 PingCAP, Inc.
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

// Code generated by plan_clone_generator; DO NOT EDIT IT DIRECTLY.

package physicalop

import (
	"github.com/pingcap/tidb/pkg/expression"
	"github.com/pingcap/tidb/pkg/planner/core/base"
	"github.com/pingcap/tidb/pkg/planner/util"
	"github.com/pingcap/tidb/pkg/planner/util/utilfuncp"
	sliceutil "github.com/pingcap/tidb/pkg/util/slice"
)

"#;
