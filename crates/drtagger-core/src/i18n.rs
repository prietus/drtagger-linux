//! User-visible strings of the services and the app. `tr` looks a message
//! up in the gettext domain "drtagger"; `trf` then fills its holes: "{}"
//! in order, or "{0}", "{1}"… by position (Rust's convention, which
//! msgfmt checks) when a translation reorders them.

pub const DOMAIN: &str = "drtagger";

/// Binds the domain to `locale_dir` and follows the user's locale. The
/// GTK app calls it once at startup; the CLI stays in English.
pub fn init(locale_dir: &std::path::Path) {
    // SAFETY: called once at startup, before any other thread exists.
    let _ = unsafe { gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "") };
    let _ = gettextrs::bindtextdomain(DOMAIN, locale_dir);
    let _ = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = gettextrs::textdomain(DOMAIN);
}

pub fn tr(message: &str) -> String {
    if message.is_empty() {
        return String::new();
    }
    gettextrs::dgettext(DOMAIN, message)
}

pub fn trf(message: &str, args: &[&dyn std::fmt::Display]) -> String {
    fill(&tr(message), args)
}

fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &after[..close];
        let index = if inner.is_empty() {
            next += 1;
            Some(next - 1)
        } else {
            inner.parse::<usize>().ok()
        };
        match index.and_then(|n| args.get(n)) {
            Some(a) => out.push_str(&a.to_string()),
            None => out.push_str(&rest[i..i + 2 + close]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::fill;

    #[test]
    fn fills_holes_in_order_and_by_position() {
        assert_eq!(fill("Disc {}: {} tracks", &[&2, &"ten"]), "Disc 2: ten tracks");
        assert_eq!(fill("{} of {}", &[&1]), "1 of {}");
        assert_eq!(fill("{1} · {0}", &[&"a", &"b"]), "b · a");
        assert_eq!(fill("{albumartist}", &[]), "{albumartist}");
    }
}
