//! Human-readable formatting for durations, counts and prices.

use std::time::Duration;

use crate::bandcamp::Price;

/// `3:07`, or `1:02:03` for an hour or more.
#[must_use]
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Album length: `38 min`, `1 h 12 min`. Rounds to the nearest minute (minimum 1).
#[must_use]
pub fn length(d: Duration) -> String {
    let minutes = ((d.as_secs() + 30) / 60).max(1);
    if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

/// `1,959,593`.
#[must_use]
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `$10.00`, `£11.99`, `¥1500`, `12.00 CAD`.
#[must_use]
pub fn price(p: &Price) -> String {
    // ISO 4217 currencies Bandcamp supports that have no minor unit.
    let zero_decimal = matches!(p.currency.as_str(), "JPY" | "KRW" | "HUF" | "ISK");
    let amount = if zero_decimal {
        p.amount_minor.to_string()
    } else {
        format!("{}.{:02}", p.amount_minor / 100, p.amount_minor % 100)
    };
    match p.currency.as_str() {
        "USD" => format!("${amount}"),
        "EUR" => format!("€{amount}"),
        "GBP" => format!("£{amount}"),
        "JPY" => format!("¥{amount}"),
        other => format!("{amount} {other}"),
    }
}

/// Shortens `s` to at most `width` terminal cells, ending in `…` if anything was cut.
/// Handles wide characters (CJK, emoji) correctly.
#[must_use]
pub fn truncate(s: &str, width: usize) -> std::borrow::Cow<'_, str> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if s.width() <= width {
        return s.into();
    }
    if width == 0 {
        return "".into();
    }
    let mut out = String::with_capacity(width + 3);
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.truncate(out.trim_end().len());
    out.push('…');
    out.into()
}

/// An error and its sources on one line: `request to discover_web failed: error sending request`.
#[must_use]
pub fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        // reqwest repeats its inner error text; skip exact duplicates.
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks() {
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
        assert_eq!(clock(Duration::from_secs(187)), "3:07");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn lengths() {
        assert_eq!(length(Duration::from_secs(10)), "1 min");
        assert_eq!(length(Duration::from_secs(2279)), "38 min");
        assert_eq!(length(Duration::from_mins(72)), "1 h 12 min");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_959_593), "1,959,593");
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate("Ocean Company", 20), "Ocean Company");
        assert_eq!(truncate("Ocean Company", 8), "Ocean C…");
        // No space left dangling before the ellipsis.
        assert_eq!(truncate("Ocean Company", 7), "Ocean…");
        assert_eq!(truncate("Ocean Company", 13), "Ocean Company");
        // Each CJK character is two cells wide.
        assert_eq!(truncate("死亡循環", 5), "死亡…");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 1), "…");
    }

    #[test]
    fn prices() {
        let p = |amount_minor, currency: &str| Price {
            amount_minor,
            currency: currency.into(),
        };
        assert_eq!(price(&p(1000, "USD")), "$10.00");
        assert_eq!(price(&p(1199, "GBP")), "£11.99");
        assert_eq!(price(&p(1500, "JPY")), "¥1500");
        assert_eq!(price(&p(1200, "CAD")), "12.00 CAD");
    }
}
