//! Looking for a word on the page (⌘F): the window's side of Search's find, from
//! `Browser.swift` (`look`, `askFind`, `findStatus`). The page's side is a script in Tap's
//! own content world (`assets/js/page-find.js`).
//!
//! One question to the page at a time: letters typed while it answers wait, and only the
//! last of them is asked next. Every question carries a generation; an answer for an
//! older one (another word, another page) is dropped.

use crate::browser::TabId;

/// What the page answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // the answer's own facts, as Search's PageFind.Result has them
pub struct Answer {
    pub count: Option<u32>,
    /// One-based, as the bar shows it.
    pub index: Option<u32>,
    /// More matches follow than were counted (the page stops at a thousand).
    pub more: bool,
    pub found: bool,
    /// A PDF: WebKit's own find, which says found or not, and no count.
    pub native: bool,
    pub available: bool,
}

/// A question for the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub tab: TabId,
    pub query: String,
    pub match_case: bool,
    pub whole_words: bool,
    /// From the first match on a new search (0 lands on it), or how far to move.
    pub steps: i32,
    pub generation: u64,
    /// Which question this is, so its answer can be matched to it.
    pub asking: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Spec {
    tab: TabId,
    query: String,
    match_case: bool,
    whole_words: bool,
}

/// The find bar's state.
#[derive(Clone, Debug, Default)]
#[allow(clippy::struct_excessive_bools)] // the bar's switches and the asking state, as Search keeps them
pub struct Find {
    open: bool,
    needle: String,
    match_case: bool,
    whole_words: bool,
    answer: Option<Answer>,
    missed: bool,
    spec: Option<Spec>,
    generation: u64,
    asking: Option<u64>,
    asked: u64,
    fresh: bool,
    steps: i32,
}

/// What the window has to do for the find: ask the page, or clear what it painted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Ask(Ask),
    Clear { tab: TabId, generation: u64 },
}

impl Find {
    #[must_use]
    pub fn open(&self) -> bool {
        self.open
    }
    #[must_use]
    pub fn needle(&self) -> &str {
        &self.needle
    }
    #[must_use]
    pub fn match_case(&self) -> bool {
        self.match_case
    }
    #[must_use]
    pub fn whole_words(&self) -> bool {
        self.whole_words
    }
    /// Something has been asked of a page for what is typed now.
    #[must_use]
    pub fn asked_for(&self) -> bool {
        self.spec.is_some()
    }
    /// The page doesn't hold what was asked for: the count turns red.
    #[must_use]
    pub fn missed(&self) -> bool {
        self.missed
    }

    /// "3 of 17", "No matches", or nothing while nothing is asked.
    #[must_use]
    pub fn status(&self) -> Option<String> {
        if self.needle.trim().is_empty() {
            return None;
        }
        let answer = self.answer.as_ref()?;
        if !answer.available {
            return Some("Search unavailable".to_owned());
        }
        if answer.native {
            return Some(
                if answer.found {
                    "Match found"
                } else {
                    "No matches"
                }
                .to_owned(),
            );
        }
        match (answer.index, answer.count) {
            (Some(index), Some(count)) if count > 0 => Some(format!(
                "{index} of {count}{}",
                if answer.more { "+" } else { "" }
            )),
            _ => Some("No matches".to_owned()),
        }
    }

    pub fn show(&mut self) {
        self.open = true;
    }

    /// What is typed. A new word is a new search, landing on its first match.
    pub fn set_needle(&mut self, needle: &str, active: Option<TabId>) -> Vec<Out> {
        if self.needle == needle {
            return Vec::new();
        }
        needle.clone_into(&mut self.needle);
        self.look(active, true)
    }

    pub fn set_match_case(&mut self, on: bool, active: Option<TabId>) -> Vec<Out> {
        if self.match_case == on {
            return Vec::new();
        }
        self.match_case = on;
        self.look(active, true)
    }

    pub fn set_whole_words(&mut self, on: bool, active: Option<TabId>) -> Vec<Out> {
        if self.whole_words == on {
            return Vec::new();
        }
        self.whole_words = on;
        self.look(active, true)
    }

    /// Return, ⌘G, or the arrows: the next match, or the one before. The same search
    /// moves from where it is; a different one starts again from the first.
    pub fn look(&mut self, active: Option<TabId>, forward: bool) -> Vec<Out> {
        let Some(tab) = active else {
            self.missed = false;
            self.answer = None;
            return Vec::new();
        };
        if self.needle.trim().is_empty() {
            self.missed = false;
            self.answer = None;
            let Some(old) = self.spec.as_ref().map(|s| s.tab) else {
                return Vec::new();
            };
            self.forget();
            return vec![Out::Clear {
                tab: old,
                generation: self.generation,
            }];
        }
        let spec = Spec {
            tab,
            query: self.needle.clone(),
            match_case: self.match_case,
            whole_words: self.whole_words,
        };
        if self.spec.as_ref() == Some(&spec) {
            self.steps += if forward { 1 } else { -1 };
        } else {
            self.generation = self.generation.wrapping_add(1);
            self.spec = Some(spec);
            self.fresh = true;
            self.steps = 0;
        }
        self.ask().into_iter().map(Out::Ask).collect()
    }

    /// Sends what is waiting, unless a question is still out: its answer sends the next.
    fn ask(&mut self) -> Option<Ask> {
        if self.asking.is_some() || !(self.fresh || self.steps != 0) {
            return None;
        }
        let spec = self.spec.clone()?;
        self.asked = self.asked.wrapping_add(1);
        self.asking = Some(self.asked);
        let steps = std::mem::take(&mut self.steps);
        self.fresh = false;
        Some(Ask {
            tab: spec.tab,
            query: spec.query,
            match_case: spec.match_case,
            whole_words: spec.whole_words,
            steps,
            generation: self.generation,
            asking: self.asked,
        })
    }

    /// The page answered question `asking`. Kept only if nothing newer took over and the
    /// same page is still in front; then whatever waited is asked.
    pub fn answered(
        &mut self,
        asking: u64,
        generation: u64,
        tab: TabId,
        answer: Answer,
        active: Option<TabId>,
    ) -> Vec<Out> {
        if self.asking != Some(asking) {
            return Vec::new();
        }
        self.asking = None;
        let current = self.spec.as_ref().is_some_and(|s| s.tab == tab) && active == Some(tab);
        if current && self.generation == generation {
            self.missed = answer.available && !answer.found;
            self.answer = Some(answer);
        }
        self.ask().into_iter().map(Out::Ask).collect()
    }

    /// Nothing asked or waiting any more; whatever answer is out is dropped.
    fn forget(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.spec = None;
        self.asking = None;
        self.fresh = false;
        self.steps = 0;
    }

    /// Esc or Done: the bar goes, what it painted on the page goes, the page's own
    /// selection comes back.
    pub fn close(&mut self) -> Vec<Out> {
        let old = self.spec.as_ref().map(|s| s.tab);
        self.forget();
        *self = Find {
            generation: self.generation,
            asked: self.asked,
            ..Find::default()
        };
        old.map(|tab| Out::Clear {
            tab,
            generation: self.generation,
        })
        .into_iter()
        .collect()
    }

    /// Another tab came to the front, or the page changed under an open bar: what was
    /// typed stays, every answer tied to the old page goes. `retry` asks the page in
    /// front again.
    pub fn moved(&mut self, active: Option<TabId>, retry: bool) -> Vec<Out> {
        let old = self.spec.as_ref().map(|s| s.tab);
        self.forget();
        self.answer = None;
        self.missed = false;
        let mut out: Vec<Out> = old
            .map(|tab| Out::Clear {
                tab,
                generation: self.generation,
            })
            .into_iter()
            .collect();
        if retry && self.open && !self.needle.trim().is_empty() {
            out.extend(self.look(active, true));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: TabId = TabId(1);

    fn asks(out: &[Out]) -> Vec<&Ask> {
        out.iter()
            .filter_map(|o| if let Out::Ask(a) = o { Some(a) } else { None })
            .collect()
    }

    fn found(count: u32, index: u32) -> Answer {
        Answer {
            count: Some(count),
            index: Some(index),
            found: count > 0,
            available: true,
            ..Answer::default()
        }
    }

    #[test]
    fn typing_asks_once_at_a_time_and_only_the_last_word_waits() {
        let mut find = Find::default();
        find.show();
        let first = find.set_needle("w", Some(TAB));
        let first = asks(&first)[0].clone();
        assert_eq!((first.query.as_str(), first.steps), ("w", 0));
        assert_eq!(
            find.set_needle("wo", Some(TAB)),
            Vec::new(),
            "one question out at a time"
        );
        assert_eq!(find.set_needle("wor", Some(TAB)), Vec::new());
        let next = find.answered(first.asking, first.generation, TAB, found(40, 1), Some(TAB));
        assert_eq!(
            asks(&next)[0].query,
            "wor",
            "only the last word typed is asked next"
        );
        assert_eq!(
            find.status(),
            None,
            "an answer for an older word is dropped"
        );
    }

    #[test]
    fn next_and_previous_move_from_where_it_is() {
        let mut find = Find::default();
        find.show();
        let ask = asks(&find.set_needle("tap", Some(TAB)))[0].clone();
        assert_eq!(
            find.answered(ask.asking, ask.generation, TAB, found(3, 1), Some(TAB)),
            Vec::new()
        );
        assert_eq!(find.status().as_deref(), Some("1 of 3"));
        let next = asks(&find.look(Some(TAB), true))[0].clone();
        assert_eq!(next.steps, 1);
        find.answered(next.asking, next.generation, TAB, found(3, 2), Some(TAB));
        let back = asks(&find.look(Some(TAB), false))[0].clone();
        assert_eq!(back.steps, -1);
    }

    #[test]
    fn nothing_found_is_said_in_red_and_more_than_counted_with_a_plus() {
        let mut find = Find::default();
        find.show();
        let ask = asks(&find.set_needle("zzz", Some(TAB)))[0].clone();
        find.answered(ask.asking, ask.generation, TAB, found(0, 0), Some(TAB));
        assert_eq!(find.status().as_deref(), Some("No matches"));
        assert!(find.missed());
        let ask = asks(&find.set_needle("e", Some(TAB)))[0].clone();
        find.answered(
            ask.asking,
            ask.generation,
            TAB,
            Answer {
                more: true,
                ..found(1000, 1)
            },
            Some(TAB),
        );
        assert_eq!(find.status().as_deref(), Some("1 of 1000+"));
        assert!(!find.missed());
    }

    #[test]
    fn closing_clears_the_page_and_another_tab_is_asked_again() {
        let mut find = Find::default();
        find.show();
        let ask = asks(&find.set_needle("tap", Some(TAB)))[0].clone();
        find.answered(ask.asking, ask.generation, TAB, found(2, 1), Some(TAB));
        let out = find.moved(Some(TabId(2)), true);
        assert!(
            matches!(out[0], Out::Clear { tab: TAB, .. }),
            "the old page is cleared"
        );
        assert_eq!(asks(&out)[0].tab, TabId(2));
        assert_eq!(find.needle(), "tap", "what was typed stays");

        let out = find.close();
        assert!(matches!(out.as_slice(), [Out::Clear { tab: TabId(2), .. }]));
        assert!(!find.open());
        assert_eq!(find.needle(), "");
    }
}
