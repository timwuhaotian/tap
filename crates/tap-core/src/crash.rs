//! A page whose process keeps dying. Search PR #849: made again twice a minute at most,
//! then left with a message and Try again, never reloaded for ever.

use std::time::{Duration, Instant};

/// How long a death counts against a page.
const WINDOW: Duration = Duration::from_secs(60);
/// How many deaths in that window are still worth making the page again for.
const LIMIT: usize = 2;

/// Recent deaths of one page's process. Tabs and the side panel's site go by the same
/// rule.
#[derive(Clone, Debug, Default)]
pub struct CrashCount {
    times: Vec<Instant>,
}

impl CrashCount {
    /// One more death. Answers whether making the page again is still worth it.
    pub fn another(&mut self, now: Instant) -> bool {
        self.times
            .retain(|then| now.saturating_duration_since(*then) < WINDOW);
        self.times.push(now);
        self.times.len() <= LIMIT
    }

    #[must_use]
    pub fn gave_up(&self) -> bool {
        self.times.len() > LIMIT
    }
}

/// Why WebKit says a page's process went away: its private
/// `_WKProcessTerminationReason`, given to
/// `_webView:webContentProcessDidTerminateWithReason:`. `Unknown` is a WebKit that gives
/// no reason, through the public callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEnd {
    Unknown,
    Memory,
    Processor,
    Asked,
    Crash,
}

impl PageEnd {
    #[must_use]
    pub fn from_webkit(reason: isize) -> PageEnd {
        match reason {
            0 => PageEnd::Memory,
            1 => PageEnd::Processor,
            2 => PageEnd::Asked,
            _ => PageEnd::Crash,
        }
    }

    /// For the log: what to look for there the next time a page gives up.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PageEnd::Unknown => "no reason given",
            PageEnd::Memory => "memory limit",
            PageEnd::Processor => "CPU limit",
            PageEnd::Asked => "on request",
            PageEnd::Crash => "crash",
        }
    }

    /// Over the page, once it has stopped being made again.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            PageEnd::Memory => "This page kept running out of memory.",
            _ => "This page kept crashing.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Search #849's `CrashCountTests`: two a minute, then it gives up; an old death
    /// stops counting after a minute.
    #[test]
    fn two_reloads_a_minute_then_it_gives_up() {
        let start = Instant::now();
        let mut count = CrashCount::default();
        assert!(count.another(start));
        assert!(count.another(start + Duration::from_secs(10)));
        assert!(!count.another(start + Duration::from_secs(20)));
        assert!(count.gave_up());

        let mut later = CrashCount::default();
        assert!(later.another(start));
        assert!(later.another(start + Duration::from_secs(30)));
        assert!(
            later.another(start + Duration::from_secs(61)),
            "the first one no longer counts"
        );
    }

    #[test]
    fn reasons_read_as_webkit_numbers_them() {
        assert_eq!(PageEnd::from_webkit(0), PageEnd::Memory);
        assert_eq!(PageEnd::from_webkit(3), PageEnd::Crash);
        assert_eq!(PageEnd::from_webkit(99), PageEnd::Crash);
        assert_eq!(
            PageEnd::Memory.message(),
            "This page kept running out of memory."
        );
    }
}
