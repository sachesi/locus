use gettextrs::gettext;
use gtk::glib;
use std::ffi::CStr;
use std::sync::OnceLock;

pub fn percent(value: f64, decimals: usize) -> String {
    let number = format!("{value:.decimals$}").replace('.', decimal_point());
    // Translators: a share of something, such as "12.3%".
    gettext("{value}%").replace("{value}", &number)
}

/// The decimal separator of the locale, which GLib writes sizes with too.
fn decimal_point() -> &'static str {
    static POINT: OnceLock<String> = OnceLock::new();
    POINT.get_or_init(|| {
        // SAFETY: localeconv returns a struct owned by the C library, whose decimal_point
        // is a NUL-terminated string; the locale is set once, before any number is shown.
        let point = unsafe { CStr::from_ptr((*libc::localeconv()).decimal_point) };
        point.to_string_lossy().into_owned()
    })
}

pub fn size(bytes: u64) -> String {
    glib::format_size(bytes).into()
}

pub fn rate(bytes_per_second: f64) -> String {
    // Translators: a transfer rate, such as "1.5 MB/s".
    gettext("{size}/s").replace("{size}", &size(bytes_per_second as u64))
}

pub fn used_of(used: u64, total: u64) -> String {
    // Translators: memory in use out of what there is, such as "5.2 GB of 16.0 GB".
    gettext("{used} of {total}")
        .replace("{used}", &size(used))
        .replace("{total}", &size(total))
}

pub fn free_of(available: u64, total: u64) -> String {
    // Translators: room left on a disk, such as "120 GB free of 500 GB".
    gettext("{available} free of {total}")
        .replace("{available}", &size(available))
        .replace("{total}", &size(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages() {
        assert_eq!(percent(12.345, 1), "12.3%");
        assert_eq!(percent(99.6, 0), "100%");
    }

    #[test]
    fn rates_and_amounts() {
        // GLib keeps a number and its unit together with a no-break space.
        assert_eq!(rate(1_500_000.0), "1.5\u{a0}MB/s");
        assert_eq!(
            used_of(5_200_000_000, 16_000_000_000),
            "5.2\u{a0}GB of 16.0\u{a0}GB"
        );
        assert_eq!(free_of(0, 1000), "0 bytes free of 1.0\u{a0}kB");
    }
}
