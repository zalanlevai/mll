use std::time::Duration;
use std::fmt::{self, Display};

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
