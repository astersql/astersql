// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// SQL `FORMAT(X, D, locale)` 使用的 locale → 数字格式规则表与格式化实现。
//
// 支持千分位分隔符、小数点符号，以及印度式分组（最右三位、左侧两位一组）；
// 仅处理内存字符串，不读系统 locale 配置。

// 维护 locale 到数字格式的映射并处理内存字符串。
use std::collections::HashMap;
use unicode_general_category::{GeneralCategory, get_general_category};

/// LocaleFormatStyle 对应 Go 的数字格式规则，字段顺序保持一致。
#[derive(Clone, Copy)]
pub struct LocaleFormatStyle {
    /// 整数部分千分位（或印度分组）分隔符；空串表示不分隔。
    pub ThousandsSep: &'static str,
    /// 小数点字符（如 `.` 或 `,`）。
    pub DecimalPoint: &'static str,
    /// true 时使用印度分组（3,2,2,...），否则每三位一组。
    pub IsIndianGrouping: bool,
}

// 格式风格 ID 与 Go 常量逐项对应，名称直接描述千分位和小数点组合。
const styleCommaDot: &str = "CommaDot";
const styleDotComma: &str = "DotComma";
const styleSpaceComma: &str = "SpaceComma";
const styleNoneComma: &str = "NoneComma";
const styleAposDot: &str = "AposDot";
const styleAposComma: &str = "AposComma";
const styleNoneDot: &str = "NoneDot";
const styleIndian: &str = "Indian";

/// formatStyleMap 对应 Go 的风格定义表；函数构造用于表达包含字符串的静态 map。
fn formatStyleMap() -> HashMap<&'static str, LocaleFormatStyle> {
    [
        (
            styleCommaDot,
            LocaleFormatStyle {
                ThousandsSep: ",",
                DecimalPoint: ".",
                IsIndianGrouping: false,
            },
        ),
        (
            styleDotComma,
            LocaleFormatStyle {
                ThousandsSep: ".",
                DecimalPoint: ",",
                IsIndianGrouping: false,
            },
        ),
        (
            styleSpaceComma,
            LocaleFormatStyle {
                ThousandsSep: " ",
                DecimalPoint: ",",
                IsIndianGrouping: false,
            },
        ),
        (
            styleNoneComma,
            LocaleFormatStyle {
                ThousandsSep: "",
                DecimalPoint: ",",
                IsIndianGrouping: false,
            },
        ),
        (
            styleAposDot,
            LocaleFormatStyle {
                ThousandsSep: "'",
                DecimalPoint: ".",
                IsIndianGrouping: false,
            },
        ),
        (
            styleAposComma,
            LocaleFormatStyle {
                ThousandsSep: "'",
                DecimalPoint: ",",
                IsIndianGrouping: false,
            },
        ),
        (
            styleNoneDot,
            LocaleFormatStyle {
                ThousandsSep: "",
                DecimalPoint: ".",
                IsIndianGrouping: false,
            },
        ),
        (
            styleIndian,
            LocaleFormatStyle {
                ThousandsSep: ",",
                DecimalPoint: ".",
                IsIndianGrouping: true,
            },
        ),
    ]
    .into_iter()
    .collect()
}

/// localeToStyleMap 保留 Go 文件中的全部小写 locale 映射；查询前仍统一转为小写。
fn localeToStyleMap() -> HashMap<&'static str, &'static str> {
    [
        ("aa_et", styleCommaDot),
        ("af_za", styleCommaDot),
        ("ak_gh", styleCommaDot),
        ("am_et", styleCommaDot),
        ("ar_ae", styleCommaDot),
        ("ar_bh", styleCommaDot),
        ("ar_dz", styleCommaDot),
        ("ar_eg", styleCommaDot),
        ("ar_in", styleCommaDot),
        ("ar_iq", styleCommaDot),
        ("ar_jo", styleCommaDot),
        ("ar_kw", styleCommaDot),
        ("ar_lb", styleCommaDot),
        ("ar_ly", styleCommaDot),
        ("ar_ma", styleCommaDot),
        ("ar_om", styleCommaDot),
        ("ar_qa", styleCommaDot),
        ("ar_sd", styleCommaDot),
        ("ar_ss", styleCommaDot),
        ("ar_sy", styleCommaDot),
        ("ar_tn", styleCommaDot),
        ("ar_ye", styleCommaDot),
        ("az_ir", styleCommaDot),
        ("bi_vu", styleCommaDot),
        ("bo_cn", styleCommaDot),
        ("bo_in", styleCommaDot),
        ("cy_gb", styleCommaDot),
        ("dv_mv", styleCommaDot),
        ("en_ag", styleCommaDot),
        ("en_au", styleCommaDot),
        ("en_bw", styleCommaDot),
        ("en_ca", styleCommaDot),
        ("en_gb", styleCommaDot),
        ("en_hk", styleCommaDot),
        ("en_ie", styleCommaDot),
        ("en_il", styleCommaDot),
        ("en_ng", styleCommaDot),
        ("en_nz", styleCommaDot),
        ("en_ph", styleCommaDot),
        ("en_sg", styleCommaDot),
        ("en_us", styleCommaDot),
        ("en_za", styleCommaDot),
        ("en_zm", styleCommaDot),
        ("en_zw", styleCommaDot),
        ("es_do", styleCommaDot),
        ("es_gt", styleCommaDot),
        ("es_hn", styleCommaDot),
        ("es_ni", styleCommaDot),
        ("es_pa", styleCommaDot),
        ("es_pr", styleCommaDot),
        ("es_sv", styleCommaDot),
        ("es_us", styleCommaDot),
        ("fa_ir", styleCommaDot),
        ("ga_ie", styleCommaDot),
        ("gd_gb", styleCommaDot),
        ("gu_in", styleCommaDot),
        ("gv_gb", styleCommaDot),
        ("ha_ng", styleCommaDot),
        ("he_il", styleCommaDot),
        ("hi_in", styleCommaDot),
        ("hy_am", styleCommaDot),
        ("ig_ng", styleCommaDot),
        ("ik_ca", styleCommaDot),
        ("iu_ca", styleCommaDot),
        ("ja_jp", styleCommaDot),
        ("km_kh", styleCommaDot),
        ("kn_in", styleCommaDot),
        ("ko_kr", styleCommaDot),
        ("ks_in", styleCommaDot),
        ("kw_gb", styleCommaDot),
        ("lg_ug", styleCommaDot),
        ("lo_la", styleCommaDot),
        ("mi_nz", styleCommaDot),
        ("mr_in", styleCommaDot),
        ("ms_my", styleCommaDot),
        ("mt_mt", styleCommaDot),
        ("my_mm", styleCommaDot),
        ("ne_np", styleCommaDot),
        ("nr_za", styleCommaDot),
        ("om_et", styleCommaDot),
        ("om_ke", styleCommaDot),
        ("pa_in", styleCommaDot),
        ("pa_pk", styleCommaDot),
        ("sa_in", styleCommaDot),
        ("sd_in", styleCommaDot),
        ("si_lk", styleCommaDot),
        ("sm_ws", styleCommaDot),
        ("so_et", styleCommaDot),
        ("so_ke", styleCommaDot),
        ("so_so", styleCommaDot),
        ("ss_za", styleCommaDot),
        ("st_za", styleCommaDot),
        ("sw_ke", styleCommaDot),
        ("sw_tz", styleCommaDot),
        ("th_th", styleCommaDot),
        ("ti_et", styleCommaDot),
        ("tk_tm", styleCommaDot),
        ("tl_ph", styleCommaDot),
        ("tn_za", styleCommaDot),
        ("to_to", styleCommaDot),
        ("ts_za", styleCommaDot),
        ("ug_cn", styleCommaDot),
        ("ur_in", styleCommaDot),
        ("ur_pk", styleCommaDot),
        ("ve_za", styleCommaDot),
        ("xh_za", styleCommaDot),
        ("yi_us", styleCommaDot),
        ("yo_ng", styleCommaDot),
        ("zh_cn", styleCommaDot),
        ("zh_hk", styleCommaDot),
        ("zh_sg", styleCommaDot),
        ("zh_tw", styleCommaDot),
        ("zu_za", styleCommaDot),
        ("an_es", styleCommaDot),
        ("az_az", styleCommaDot),
        ("ca_ad", styleCommaDot),
        ("ca_fr", styleCommaDot),
        ("ca_it", styleCommaDot),
        ("de_it", styleCommaDot),
        ("en_dk", styleCommaDot),
        ("es_pe", styleCommaDot),
        ("ff_sn", styleCommaDot),
        ("fy_de", styleCommaDot),
        ("fy_nl", styleCommaDot),
        ("ka_ge", styleCommaDot),
        ("kl_gl", styleCommaDot),
        ("ku_tr", styleCommaDot),
        ("lb_lu", styleCommaDot),
        ("li_be", styleCommaDot),
        ("li_nl", styleCommaDot),
        ("nl_aw", styleCommaDot),
        ("sc_it", styleCommaDot),
        ("se_no", styleCommaDot),
        ("sq_mk", styleCommaDot),
        ("tg_tj", styleCommaDot),
        ("tr_cy", styleCommaDot),
        ("wa_be", styleCommaDot),
        ("br_fr", styleCommaDot),
        ("kk_kz", styleCommaDot),
        ("nn_no", styleCommaDot),
        ("oc_fr", styleCommaDot),
        ("uz_uz", styleCommaDot),
        ("bs_ba", styleCommaDot),
        ("el_cy", styleCommaDot),
        ("es_cu", styleCommaDot),
        ("ln_cd", styleCommaDot),
        ("mg_mg", styleCommaDot),
        ("rw_rw", styleCommaDot),
        ("sr_me", styleCommaDot),
        ("wo_sn", styleCommaDot),
        ("es_mx", styleCommaDot),
        ("ce_ru", styleCommaDot),
        ("cv_ru", styleCommaDot),
        ("ht_ht", styleCommaDot),
        ("ia_fr", styleCommaDot),
        ("ky_kg", styleCommaDot),
        ("os_ru", styleCommaDot),
        ("tt_ru", styleCommaDot),
        ("aa_dj", styleCommaDot),
        ("aa_er", styleCommaDot),
        ("so_dj", styleCommaDot),
        ("ti_er", styleCommaDot),
        ("ps_af", styleCommaDot),
        ("kv_ru", styleCommaDot),
        ("su_id", styleCommaDot),
        // styleDotComma (123.456,78): Common in Europe and South America.
        // 千分位用点、小数点用逗号（欧洲/南美常见）。
        ("be_by", styleDotComma),
        ("da_dk", styleDotComma),
        ("de_be", styleDotComma),
        ("de_de", styleDotComma),
        ("de_lu", styleDotComma),
        ("es_ar", styleDotComma),
        ("es_bo", styleDotComma),
        ("es_cl", styleDotComma),
        ("es_co", styleDotComma),
        ("es_ec", styleDotComma),
        ("es_es", styleDotComma),
        ("es_py", styleDotComma),
        ("es_uy", styleDotComma),
        ("es_ve", styleDotComma),
        ("fo_fo", styleDotComma),
        ("hu_hu", styleDotComma),
        ("id_id", styleDotComma),
        ("is_is", styleDotComma),
        ("lt_lt", styleDotComma),
        ("mn_mn", styleDotComma),
        ("ro_ro", styleDotComma),
        ("ru_ua", styleDotComma),
        ("sq_al", styleDotComma),
        ("tr_tr", styleDotComma),
        ("vi_vn", styleDotComma),
        ("nb_no", styleDotComma),
        ("uk_ua", styleDotComma),
        ("no_no", styleDotComma),
        // styleSpaceComma (123 456,78): Uses space as thousands separator.
        // 千分位为空格、小数点为逗号。
        ("cs_cz", styleSpaceComma),
        ("es_cr", styleSpaceComma),
        ("et_ee", styleSpaceComma),
        ("fi_fi", styleSpaceComma),
        ("lv_lv", styleSpaceComma),
        ("mk_mk", styleSpaceComma),
        ("ru_ru", styleSpaceComma),
        ("sk_sk", styleSpaceComma),
        ("sv_fi", styleSpaceComma),
        ("sv_se", styleSpaceComma),
        // styleNoneComma (123456,78): No thousands separator.
        // 无千分位，小数点为逗号。
        ("el_gr", styleNoneComma),
        ("gl_es", styleNoneComma),
        ("pt_pt", styleNoneComma),
        ("sl_si", styleNoneComma),
        ("ca_es", styleNoneComma),
        ("de_at", styleNoneComma),
        ("eu_es", styleNoneComma),
        ("fr_be", styleNoneComma),
        ("hr_hr", styleNoneComma),
        ("it_it", styleNoneComma),
        ("nl_be", styleNoneComma),
        ("nl_nl", styleNoneComma),
        ("pt_br", styleNoneComma),
        ("fr_ca", styleNoneComma),
        ("fr_fr", styleNoneComma),
        ("fr_lu", styleNoneComma),
        ("pl_pl", styleNoneComma),
        ("fr_ch", styleNoneComma),
        ("bg_bg", styleNoneComma),
        // styleAposDot (123'456.78): Uses apostrophe as thousands separator.
        // 千分位为撇号、小数点为点。
        ("de_ch", styleAposDot),
        // styleAposComma (123'456,78): Uses apostrophe separator, comma decimal.
        // 千分位为撇号、小数点为逗号。
        ("it_ch", styleAposComma),
        // styleNoneDot (123456.78): No thousands separator, dot decimal.
        // 无千分位，小数点为点。
        ("ar_sa", styleNoneDot),
        ("sr_rs", styleNoneDot),
        // styleIndian (1,23,45,67,890.123): Special Indian grouping (3,2,2,...).
        // 印度式分组：最右三位，左侧每两位一组。
        ("en_in", styleIndian),
        ("ta_in", styleIndian),
        ("te_in", styleIndian),
    ]
    .into_iter()
    .collect()
}

/// GetLocaleFormatStyle 返回 locale 的格式规则和是否命中映射。
/// 未命中时仍返回 en_US 对应的逗号/点默认风格，但 found 为 false。
pub fn GetLocaleFormatStyle(locale: &str) -> (LocaleFormatStyle, bool) {
    let locale_key = locale.to_lowercase();
    let locale_map = localeToStyleMap();
    let styles = formatStyleMap();
    match locale_map.get(locale_key.as_str()) {
        Some(style_id) => (styles[style_id], true),
        None => (styles[styleCommaDot], false),
    }
}

/// FormatByLocale 对应 Go 的公开入口，保留“格式化结果、locale 是否命中、错误”三部分语义。
/// 当前算法没有外部失败源；错误类型只承接精度解析形状，解析失败仍按 Go 逻辑忽略。
pub fn FormatByLocale(
    number: &str,
    precision: &str,
    locale: &str,
) -> (String, bool, Result<(), std::num::ParseIntError>) {
    let (style, found) = GetLocaleFormatStyle(locale);
    let formatted = formatWithStyle(number, precision, style);
    (formatted, found, Ok(()))
}

/// formatWithStandardGrouping 按从左到右每三位插入分隔符，例如 1,234,567。
fn formatWithStandardGrouping(integer_part: &str, thousands_sep: &str) -> String {
    let part_len = integer_part.len();
    let mut pos = part_len % 3;
    if pos == 0 && part_len > 0 {
        pos = 3;
    }

    let mut buffer = String::new();
    // Go 按字节切 ASCII 数字；这里保持相同前提和首组 1～3 位规则。
    buffer.push_str(&integer_part[..pos]);
    while pos < part_len {
        buffer.push_str(thousands_sep);
        buffer.push_str(&integer_part[pos..pos + 3]);
        pos += 3;
    }
    buffer
}
/// formatWithIndianGrouping 保留最右三位，左侧按两位分组，例如 1,23,45,67,890。
fn formatWithIndianGrouping(integer_part: &str, thousands_sep: &str) -> String {
    let len = integer_part.len();
    if len <= 3 {
        return integer_part.to_owned();
    }

    let rightmost3 = &integer_part[len - 3..];
    let remaining = &integer_part[..len - 3];
    let rem_len = remaining.len();
    let mut first_part_len = rem_len % 2;
    if first_part_len == 0 && rem_len > 0 {
        first_part_len = 2;
    }

    let mut buffer = String::new();
    if first_part_len > 0 {
        buffer.push_str(&remaining[..first_part_len]);
    }
    let mut pos = first_part_len;
    while pos < rem_len {
        buffer.push_str(thousands_sep);
        buffer.push_str(&remaining[pos..pos + 2]);
        pos += 2;
    }
    buffer.push_str(thousands_sep);
    buffer.push_str(rightmost3);
    buffer
}

/// formatWithStyle 对应 Go 的通用格式化流程。
/// 输入由 SQL FORMAT 调用方提供，Go 代码按 ASCII 字节索引；显式保留该约束并避免外部 IO。
fn formatWithStyle(number: &str, precision: &str, style: LocaleFormatStyle) -> String {
    // 精度只接受开头连续的十进制数字；非数字开头等价于精度 0。
    let precision_prefix: String = precision
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    let precision_text = if precision_prefix.is_empty() {
        "0"
    } else {
        &precision_prefix
    };
    let position = precision_text.parse::<usize>().ok();

    // 规范化前导小数点：`-.5` → `-0.5`，`.5` → `0.5`，与 Go 行为一致。
    let mut normalized = number.to_owned();
    if normalized.starts_with("-.") {
        normalized.insert(1, '0');
    } else if normalized.starts_with('.') {
        normalized.insert(0, '0');
    }

    let bytes = normalized.as_bytes();
    let valid_start = bytes.first().is_some_and(u8::is_ascii_digit)
        || (bytes.first() == Some(&b'-') && bytes.get(1).is_some_and(u8::is_ascii_digit));
    // 非数字输入回退为 `0` 并按精度补零小数位。
    if !valid_start {
        let mut buffer = String::from("0");
        if let Some(position) = position.filter(|value| *value > 0) {
            buffer.push_str(style.DecimalPoint);
            buffer.push_str(&"0".repeat(position));
        }
        return buffer;
    }

    let negative = normalized.starts_with('-');
    let unsigned = if negative {
        &normalized[1..]
    } else {
        &normalized
    };
    // 与 Go 循环一致：当第二个字节不是小数点时，后续每个小数点也会
    // 被保留；最终只有恰好拆成两段时才使用小数部分。
    let mut end = unsigned.len();
    for (index, ch) in unsigned.char_indices() {
        if get_general_category(ch) == GeneralCategory::DecimalNumber {
            continue;
        }
        if index == 1 && unsigned.as_bytes().get(1) == Some(&b'.') {
            continue;
        }
        if ch == '.' && unsigned.as_bytes().get(1) != Some(&b'.') {
            continue;
        }
        end = index;
        break;
    }
    let parsed = &unsigned[..end];
    let parts = parsed.split('.').collect::<Vec<_>>();
    let integer_part = parts.first().copied().unwrap_or_default();
    let fraction_part = (parts.len() == 2).then(|| parts[1]);

    let formatted_integer = if style.ThousandsSep.is_empty() {
        integer_part.to_owned()
    } else if style.IsIndianGrouping {
        formatWithIndianGrouping(integer_part, style.ThousandsSep)
    } else {
        formatWithStandardGrouping(integer_part, style.ThousandsSep)
    };

    let mut buffer = String::new();
    if negative {
        buffer.push('-');
    }
    buffer.push_str(&formatted_integer);

    if let Some(position) = position.filter(|value| *value > 0) {
        buffer.push_str(style.DecimalPoint);
        let fraction = fraction_part.unwrap_or_default();
        if fraction.len() >= position {
            buffer.push_str(&fraction[..position]);
        } else {
            buffer.push_str(fraction);
            buffer.push_str(&"0".repeat(position - fraction.len()));
        }
    }
    buffer
}
