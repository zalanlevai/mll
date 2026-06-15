use std::time::Duration;
use std::fmt::{self, Display};

#[derive(Copy, Clone, Debug)]
pub enum MemoryUnit {
    Bytes,
    KiB,
    MiB,
    GiB,
}

pub struct MemoryDisplay(u64, Option<MemoryUnit>);

impl Display for MemoryDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let memory_unit = self.1.unwrap_or_else(|| {
            let mem_in_kib = self.0 as f64 / 1024_f64;
            if mem_in_kib < 1_f64 { return MemoryUnit::Bytes; }

            let mem_in_mib = mem_in_kib / 1024_f64;
            if mem_in_mib < 1_f64 { return MemoryUnit::KiB; }

            let mem_in_gib = mem_in_mib / 1024_f64;
            if mem_in_gib < 1_f64 { return MemoryUnit::MiB; }

            MemoryUnit::GiB
        });

        let precision = f.precision().unwrap_or(1);

        match memory_unit {
            MemoryUnit::Bytes if self.0 == 1 => write!(f, "1 byte"),
            MemoryUnit::Bytes => write!(f, "{} bytes", self.0),
            MemoryUnit::KiB => write!(f, "{:.precision$} KiB", self.0 as f64 / 1024_f64),
            MemoryUnit::MiB => write!(f, "{:.precision$} MiB", self.0 as f64 / 1024_f64 / 1024_f64),
            MemoryUnit::GiB => write!(f, "{:.precision$} GiB", self.0 as f64 / 1024_f64 / 1024_f64 / 1024_f64),
        }
    }
}

pub trait MemoryDisplayExt {
    fn display_memory(&self) -> MemoryDisplay;
    fn display_memory_in(&self, memory_unit: MemoryUnit) -> MemoryDisplay;
}

impl MemoryDisplayExt for u64 {
    fn display_memory(&self) -> MemoryDisplay {
        MemoryDisplay(*self, None)
    }

    fn display_memory_in(&self, memory_unit: MemoryUnit) -> MemoryDisplay {
        MemoryDisplay(*self, Some(memory_unit))
    }
}

pub struct DurationDisplayImpreciseAgo(Duration);

impl Display for DurationDisplayImpreciseAgo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_secs() {
            0 => write!(f, "just now"),

            1 => write!(f, "1 second ago"),
            v @ ..60 => write!(f, "{} seconds ago", v),

            60..120 => write!(f, "1 minute ago"),
            v @ ..3600 => write!(f, "{} minutes ago", v / 60),

            3600..7200 => write!(f, "1 hour ago"),
            v => write!(f, "{} hours ago", v / 3600),
        }
    }
}

pub trait DurationDisplayExt {
    fn display_imprecise_ago(&self) -> DurationDisplayImpreciseAgo;
}

impl DurationDisplayExt for Duration {
    fn display_imprecise_ago(&self) -> DurationDisplayImpreciseAgo {
        DurationDisplayImpreciseAgo(*self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_display() {
        assert_eq!("0 bytes", format!("{}", 0.display_memory_in(MemoryUnit::Bytes)));
        assert_eq!("1 byte", format!("{}", 1.display_memory_in(MemoryUnit::Bytes)));
        assert_eq!("8 bytes", format!("{}", 8.display_memory_in(MemoryUnit::Bytes)));
        assert_eq!("102641958912 bytes", format!("{}", 102641958912.display_memory_in(MemoryUnit::Bytes)));

        assert_eq!("0.8 KiB", format!("{}", 819.display_memory_in(MemoryUnit::KiB)));
        assert_eq!("1.3 KiB", format!("{}", 1380.display_memory_in(MemoryUnit::KiB)));
        assert_eq!("100236288.0 KiB", format!("{}", 102641958912.display_memory_in(MemoryUnit::KiB)));

        assert_eq!("0.8 MiB", format!("{}", (819 * 1024).display_memory_in(MemoryUnit::MiB)));
        assert_eq!("1.3 MiB", format!("{}", (1380 * 1024).display_memory_in(MemoryUnit::MiB)));
        assert_eq!("97887.0 MiB", format!("{}", 102641958912.display_memory_in(MemoryUnit::MiB)));

        assert_eq!("0.8 GiB", format!("{}", (819 * 1024 * 1024).display_memory_in(MemoryUnit::GiB)));
        assert_eq!("1.3 GiB", format!("{}", (1380 * 1024 * 1024).display_memory_in(MemoryUnit::GiB)));
        assert_eq!("95.6 GiB", format!("{}", 102641958912.display_memory_in(MemoryUnit::GiB)));

        assert_eq!("819 bytes", format!("{}", 819.display_memory()));
        assert_eq!("1.0 KiB", format!("{}", 1024.display_memory()));
        assert_eq!("1023.0 KiB", format!("{}", (1024 * 1023).display_memory()));
        assert_eq!("1.0 MiB", format!("{}", (1024 * 1024).display_memory()));
        assert_eq!("1023.0 MiB", format!("{}", (1024 * 1024 * 1023).display_memory()));
        assert_eq!("1.0 GiB", format!("{}", (1024 * 1024 * 1024).display_memory()));
        assert_eq!("96.0 GiB", format!("{}", (96 * 1024 * 1024 * 1024).display_memory()));

        assert_eq!("96.000 GiB", format!("{:.3}", (96 * 1024 * 1024 * 1024).display_memory()));
    }

    #[test]
    fn test_duration_display_imprecise_ago() {
        assert_eq!("just now", format!("{}", Duration::from_secs(0).display_imprecise_ago()));
        assert_eq!("just now", format!("{}", Duration::from_millis(900).display_imprecise_ago()));

        assert_eq!("1 second ago", format!("{}", Duration::from_secs(1).display_imprecise_ago()));
        assert_eq!("1 second ago", format!("{}", Duration::from_millis(1_900).display_imprecise_ago()));

        assert_eq!("2 seconds ago", format!("{}", Duration::from_secs(2).display_imprecise_ago()));
        assert_eq!("28 seconds ago", format!("{}", Duration::from_millis(28_999).display_imprecise_ago()));
        assert_eq!("59 seconds ago", format!("{}", Duration::from_millis(59_999).display_imprecise_ago()));

        assert_eq!("1 minute ago", format!("{}", Duration::from_secs(60).display_imprecise_ago()));
        assert_eq!("1 minute ago", format!("{}", Duration::from_secs(119).display_imprecise_ago()));

        assert_eq!("2 minutes ago", format!("{}", Duration::from_secs(2 * 60).display_imprecise_ago()));
        assert_eq!("28 minutes ago", format!("{}", Duration::from_secs(28 * 60 + 59).display_imprecise_ago()));
        assert_eq!("59 minutes ago", format!("{}", Duration::from_secs(59 * 60 + 59).display_imprecise_ago()));

        assert_eq!("1 hour ago", format!("{}", Duration::from_secs(3600).display_imprecise_ago()));
        assert_eq!("1 hour ago", format!("{}", Duration::from_secs(7199).display_imprecise_ago()));

        assert_eq!("2 hours ago", format!("{}", Duration::from_secs(2 * 3600).display_imprecise_ago()));
        assert_eq!("28 hours ago", format!("{}", Duration::from_secs(28 * 3600 + 3599).display_imprecise_ago()));
        assert_eq!("59 hours ago", format!("{}", Duration::from_secs(59 * 3600 + 3599).display_imprecise_ago()));

        assert_eq!("100 hours ago", format!("{}", Duration::from_secs(100 * 3600).display_imprecise_ago()));
        assert_eq!("1000 hours ago", format!("{}", Duration::from_secs(1000 * 3600).display_imprecise_ago()));
    }
}
