use gettextrs::{gettext, ngettext};
use gtk::glib;
use std::ffi::CStr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether sizes are counted in 1024s, as KiB, MiB and GiB, rather than in 1000s.
static BINARY: AtomicBool = AtomicBool::new(true);

pub fn set_binary(binary: bool) {
    BINARY.store(binary, Ordering::Relaxed);
}

pub fn is_binary() -> bool {
    BINARY.load(Ordering::Relaxed)
}

pub fn percent(value: f64, decimals: usize) -> String {
    // Translators: a share of something, such as "12.3%".
    gettext("{value}%").replace("{value}", &number(value, decimals))
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

/// Two short values side by side, such as "55%, 17.3 GiB".
pub fn listed(first: &str, second: &str) -> String {
    // Translators: two short values side by side, such as "55%, 17.3 GiB".
    gettext("{first}, {second}")
        .replace("{first}", first)
        .replace("{second}", second)
}

/// `used` as a percentage of `total`.
pub fn share(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    }
}

pub fn size(bytes: u64) -> String {
    size_in(bytes, is_binary())
}

fn size_in(bytes: u64, binary: bool) -> String {
    let flags = if binary {
        glib::FormatSizeFlags::IEC_UNITS
    } else {
        glib::FormatSizeFlags::DEFAULT
    };
    glib::format_size_full(bytes, flags).into()
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

pub fn temperature(celsius: f64) -> String {
    // Translators: a temperature in degrees Celsius, such as "45 °C".
    gettext("{value} °C").replace("{value}", &format!("{celsius:.0}"))
}

/// A clock speed, in the unit that fits it.
pub fn speed(hertz: f64) -> String {
    let (value, text) = if hertz >= 1e9 {
        // Translators: a clock speed, such as "3.84 GHz".
        (hertz / 1e9, gettext("{value} GHz"))
    } else {
        // Translators: a clock speed, such as "800 MHz".
        (hertz / 1e6, gettext("{value} MHz"))
    };
    let decimals = if hertz >= 1e9 { 2 } else { 0 };
    text.replace("{value}", &number(value, decimals))
}

pub fn watts(watts: f64) -> String {
    // Translators: power drawn, such as "118 W".
    gettext("{value} W").replace("{value}", &number(watts, 0))
}

pub fn rpm(rpm: u64) -> String {
    // Translators: how fast a fan turns, such as "1240 RPM".
    gettext("{value} RPM").replace("{value}", &rpm.to_string())
}

pub fn milliseconds(ms: f64) -> String {
    // Translators: a short time, such as "0.4 ms".
    gettext("{value} ms").replace("{value}", &number(ms, 1))
}

/// How long something has been running, as hours, minutes and seconds, with days before them
/// once there are any.
pub fn uptime(seconds: u64) -> String {
    let (minutes, secs) = (seconds / 60, seconds % 60);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    let (days, hours) = (hours / 24, hours % 24);
    if days == 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        let days = u32::try_from(days).unwrap_or(u32::MAX);
        // Translators: time since the machine started, such as "2 days 3:14:07".
        ngettext("{days} day {time}", "{days} days {time}", days)
            .replace("{days}", &days.to_string())
            .replace("{time}", &format!("{hours}:{minutes:02}:{secs:02}"))
    }
}

pub fn number(value: f64, decimals: usize) -> String {
    format!("{value:.decimals$}").replace('.', decimal_point())
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
        assert_eq!(rate(1_572_864.0), "1.5\u{a0}MiB/s");
        assert_eq!(
            used_of(5_583_457_485, 17_179_869_184),
            "5.2\u{a0}GiB of 16.0\u{a0}GiB"
        );
        assert_eq!(free_of(0, 1024), "0 bytes free of 1.0\u{a0}KiB");
        assert_eq!(size_in(1_500_000, false), "1.5\u{a0}MB");
        assert_eq!(temperature(44.6), "45 °C");
        assert_eq!(speed(3_840_000_000.0), "3.84 GHz");
        assert_eq!(speed(800_000_000.0), "800 MHz");
        assert_eq!(uptime(11_647), "3:14:07");
        assert_eq!(uptime(90_061), "1 day 1:01:01");
    }
}
