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

use serde::{Deserialize, Serialize};

/// ENGINE_ATTRIBUTE 的 JSON 形式。
#[derive(Clone, Debug, Default, Serialize)]
pub struct EngineAttribute {
    #[serde(rename = "storage_class")]
    pub StorageClass: Option<Box<serde_json::value::RawValue>>,
}

impl PartialEq for EngineAttribute {
    fn eq(&self, other: &Self) -> bool {
        self.StorageClass.as_ref().map(|raw| raw.get())
            == other.StorageClass.as_ref().map(|raw| raw.get())
    }
}

impl<'de> Deserialize<'de> for EngineAttribute {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = EngineAttribute;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("engine attribute object")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(EngineAttribute::default())
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut attribute = EngineAttribute::default();
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("storage_class") {
                        attribute.StorageClass = Some(map.next_value()?);
                    } else {
                        let _: serde::de::IgnoredAny = map.next_value()?;
                    }
                }
                Ok(attribute)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// 空输入代表零值属性；非空输入必须是合法对象 JSON。
pub fn ParseEngineAttributeFromString(input: &str) -> Result<EngineAttribute, serde_json::Error> {
    if input.is_empty() || input.trim() == "null" {
        return Ok(EngineAttribute::default());
    }
    serde_json::from_str(input)
}

pub const StorageClassTierStandard: &str = "STANDARD";
pub const StorageClassTierIA: &str = "IA";
pub const StorageClassTierDefault: &str = StorageClassTierStandard;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageClassDef {
    #[serde(rename = "tier", default)]
    pub Tier: String,
    #[serde(rename = "names_in", default)]
    pub NamesIn: Option<Vec<String>>,
    #[serde(rename = "less_than", default)]
    pub LessThan: Option<String>,
    #[serde(rename = "values_in", default)]
    pub ValuesIn: Option<Vec<String>>,
    #[serde(rename = "transitions", default)]
    pub Transitions: Option<Vec<StorageClassTransitRule>>,
}
impl StorageClassDef {
    pub fn HasNoScopeDef(&self) -> bool {
        self.NamesIn.as_ref().is_none_or(Vec::is_empty)
            && self.LessThan.is_none()
            && self.ValuesIn.as_ref().is_none_or(Vec::is_empty)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageClassSettings {
    #[serde(rename = "defs", default)]
    pub Defs: Option<Vec<Option<StorageClassDef>>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageClassTransitRule {
    #[serde(rename = "tier", default)]
    pub Tier: String,
    #[serde(rename = "after_days", default)]
    pub AfterDays: u64,
    #[serde(rename = "after_seconds", default)]
    pub AfterSeconds: u64,
}
impl StorageClassTransitRule {
    pub fn TotalSeconds(&self) -> u64 {
        self.AfterDays
            .wrapping_mul(86400)
            .wrapping_add(self.AfterSeconds)
    }
}

pub fn buildStorageClassString(tier: &str, transitions: &[StorageClassTransitRule]) -> String {
    if transitions.is_empty() {
        return tier.to_owned();
    }
    #[derive(Serialize)]
    struct StorageClassInfo<'a> {
        tier: &'a str,
        transitions: &'a [StorageClassTransitRule],
    }
    serde_json::to_string(&StorageClassInfo { tier, transitions }).unwrap_or_default()
}
