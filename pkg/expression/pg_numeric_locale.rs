// Copyright 2026 AsterSQL.
//! Owned numeric/monetary locale facts. Never changes the process locale.
use std::ffi::{CStr, CString};
use types_dependency::pg_numeric::PgNumericError;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn localeconv_l(locale: libc::locale_t) -> *mut libc::lconv;
    fn nl_langinfo_l(item: libc::nl_item, locale: libc::locale_t) -> *mut libc::c_char;
}

struct Locale(libc::locale_t);
impl Drop for Locale {
    fn drop(&mut self) {
        // SAFETY: this object uniquely owns a successful newlocale allocation.
        unsafe {
            libc::freelocale(self.0);
        }
    }
}
impl Locale {
    fn new(name: &str, monetary: bool) -> Result<Self, PgNumericError> {
        let name = CString::new(name).map_err(|_| PgNumericError::InvalidModifier)?;
        if name.as_bytes().is_empty() {
            return Err(PgNumericError::InvalidModifier);
        }
        let mask = if monetary {
            libc::LC_MONETARY_MASK
        } else {
            libc::LC_NUMERIC_MASK
        };
        // SAFETY: terminated name, fresh independent allocation; CTYPE identifies
        // the encoding of symbols without touching any process-global category.
        let handle = unsafe {
            libc::newlocale(
                mask | libc::LC_CTYPE_MASK,
                name.as_ptr(),
                std::ptr::null_mut(),
            )
        };
        if handle.is_null() {
            Err(PgNumericError::InvalidModifier)
        } else {
            Ok(Self(handle))
        }
    }
    fn read(
        &self,
        extract: impl FnOnce(&libc::lconv) -> Vec<*const libc::c_char>,
    ) -> Result<Vec<String>, PgNumericError> {
        #[cfg(not(target_os = "macos"))]
        struct Restore(libc::locale_t);
        #[cfg(not(target_os = "macos"))]
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    libc::uselocale(self.0);
                }
            }
        }
        // SAFETY: the locale is live for the whole read. Darwin provides the
        // direct per-locale API. Other POSIX platforms temporarily select only
        // this thread's locale, restoring it before the owned handle is freed.
        unsafe {
            #[cfg(target_os = "macos")]
            let (conv, codeset) = (localeconv_l(self.0), nl_langinfo_l(libc::CODESET, self.0));
            // Some libc implementations return a shared localeconv buffer.
            // Keep its use serialized until all symbols have been copied.
            #[cfg(not(target_os = "macos"))]
            static CONVERSION: std::sync::Mutex<()> = std::sync::Mutex::new(());
            #[cfg(not(target_os = "macos"))]
            let _conversion = CONVERSION
                .lock()
                .map_err(|_| PgNumericError::InvalidModifier)?;
            #[cfg(not(target_os = "macos"))]
            let (conv, codeset, _restore) = {
                let previous = libc::uselocale(self.0);
                if previous.is_null() {
                    return Err(PgNumericError::InvalidModifier);
                }
                (
                    libc::localeconv(),
                    libc::nl_langinfo(libc::CODESET),
                    Restore(previous),
                )
            };
            let encoding = encoding_rs::Encoding::for_label(CStr::from_ptr(codeset).to_bytes())
                .ok_or(PgNumericError::InvalidModifier)?;
            extract(&*conv)
                .into_iter()
                .map(|symbol| {
                    let bytes = CStr::from_ptr(symbol).to_bytes();
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        return Ok(text.into());
                    }
                    let (text, _, errors) = encoding.decode(bytes);
                    if errors {
                        Err(PgNumericError::InvalidModifier)
                    } else {
                        Ok(text.into_owned())
                    }
                })
                .collect()
        }
    }
}

pub fn validate(name: &str, monetary: bool) -> bool {
    Locale::new(name, monetary).is_ok()
}

pub(crate) struct Symbols {
    pub decimal: String,
    pub group: String,
    pub currency: String,
    pub positive: String,
    pub negative: String,
}
/// The fallback strings match PostgreSQL NUM_prepare_locale, including C's blank currency.
pub(crate) fn symbols(names: &[String; 2]) -> Result<Symbols, PgNumericError> {
    let mut numeric = Locale::new(&names[0], false)?
        .read(|c| vec![c.decimal_point, c.thousands_sep])?
        .into_iter();
    let mut monetary = Locale::new(&names[1], true)?
        .read(|c| vec![c.currency_symbol, c.positive_sign, c.negative_sign])?
        .into_iter();
    let fallback = |text: String, default: &str| {
        if text.is_empty() {
            default.into()
        } else {
            text
        }
    };
    let decimal = fallback(numeric.next().unwrap(), ".");
    let group = fallback(
        numeric.next().unwrap(),
        if decimal == "," { "." } else { "," },
    );
    Ok(Symbols {
        decimal,
        group,
        currency: fallback(monetary.next().unwrap(), " "),
        positive: fallback(monetary.next().unwrap(), "+"),
        negative: fallback(monetary.next().unwrap(), "-"),
    })
}
