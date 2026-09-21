//! What closing the window means.
//!
//! Three answers, and the question that has to be asked once before any of them
//! can be remembered. What separates them is what happens to the browsers and the
//! Xray tunnels this program started: they are its children, and every other exit
//! path in this repository exists to make sure they are not left behind by
//! accident. This is the one place where leaving them behind is the *request*.
//!
//! - **Background.** The window goes away and the program keeps running, with a
//!   tray icon to bring the window back and to leave. Nothing is stopped, so
//!   nothing has to be recorded: the supervisor is still there, holding its
//!   children exactly as it was before the window closed.
//! - **Keep running.** The program ends and the browsers and tunnels do not. The
//!   supervisor detaches them - dropping the handles without killing anything -
//!   and marks their session records, so the next start can tell "left here on
//!   purpose" from "left here by a crash".
//! - **Exit all.** The program ends and everything it started ends with it. This
//!   is what the program has always done, and it is what a signal does.
//!
//! **A marked record is adopted, not reclaimed.** The rule that a start reclaims
//! what a previous run left exists because a browser that survived a crash still
//! holds its profile directory and its debugging port, and a new start must not
//! race it. A browser left on purpose holds exactly the same things, so the
//! answer cannot be to stop it - that would make "keep running" mean "keep running
//! until you open the window again". It is taken over instead: it becomes a
//! running profile again, with its ports and its processes, and stopping it is an
//! ordinary stop. An unmarked record is still reclaimed, so a crash is cleaned up
//! exactly as it was before any of this existed.

use crate::text::Text;

/// The stored answer to "what happens when the window is closed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExitMode {
    /// Ask, with a dialog, every time.
    ///
    /// The default, because the other three decide what happens to processes the
    /// user is looking at, and a program that guesses once and never asks again is
    /// a program that eventually stops a browser someone was using.
    #[default]
    Ask,
    /// Keep running, with no window and a tray icon.
    Background,
    /// Leave the program; leave the browsers and tunnels running.
    KeepRunning,
    /// Leave the program; stop everything it started.
    ExitAll,
}

/// What a close does, once it has been decided.
///
/// The variant names are [`ExitMode`]'s, deliberately: the two lists are the same
/// four words minus the question, and a second set of names for the same three
/// answers would be one more place for them to drift apart.
///
/// The same three, without the question: this is what [`ExitMode::choice`]
/// answers when the mode is not [`ExitMode::Ask`], and what the dialog produces
/// when it is.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Background,
    KeepRunning,
    ExitAll,
}

impl ExitMode {
    /// Every choice, in the order the settings card shows them.
    pub const ALL: [ExitMode; 4] = [
        Self::Ask,
        Self::Background,
        Self::KeepRunning,
        Self::ExitAll,
    ];

    /// The name it is stored under.
    pub fn code(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Background => "background",
            Self::KeepRunning => "keep-running",
            Self::ExitAll => "exit-all",
        }
    }

    /// Reads a stored name, falling back to asking for anything it does not know.
    ///
    /// Falling back rather than refusing, like the appearance: a config file
    /// written by a later build that names a fourth mode must still start. Asking
    /// is the safe fallback, because the other three answer a question the user
    /// has not actually been asked.
    pub fn from_code(code: &str) -> Self {
        match code.trim().to_ascii_lowercase().as_str() {
            "background" => Self::Background,
            "keep-running" => Self::KeepRunning,
            "exit-all" => Self::ExitAll,
            _ => Self::Ask,
        }
    }

    /// What the switch calls it, in the language the window is speaking.
    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::Ask => t.exit_ask,
            Self::Background => t.exit_background,
            Self::KeepRunning => t.exit_keep_running,
            Self::ExitAll => t.exit_exit_all,
        }
    }

    /// One line saying what it does, for the settings card and the dialog.
    pub fn note(self, t: &Text) -> &'static str {
        match self {
            Self::Ask => t.exit_ask_note,
            Self::Background => t.exit_background_note,
            Self::KeepRunning => t.exit_keep_running_note,
            Self::ExitAll => t.exit_exit_all_note,
        }
    }

    /// The exit this mode asks for, or `None` when the mode is "ask".
    pub fn choice(self) -> Option<Exit> {
        match self {
            Self::Ask => None,
            Self::Background => Some(Exit::Background),
            Self::KeepRunning => Some(Exit::KeepRunning),
            Self::ExitAll => Some(Exit::ExitAll),
        }
    }
}

impl Exit {
    /// What the dialog calls it, and the code the chip's id is built from. Both
    /// come from the mode it remembers, so the two lists cannot drift.
    pub fn label(self, t: &Text) -> &'static str {
        self.mode().label(t)
    }

    pub fn note(self, t: &Text) -> &'static str {
        self.mode().note(t)
    }

    pub fn code(self) -> &'static str {
        self.mode().code()
    }

    /// The mode that remembers this answer.
    ///
    /// One direction only: [`ExitMode::choice`] turns a mode into the exit it
    /// means, and this turns an exit choice back into the mode "remember this"
    /// stores. `Ask` is deliberately not reachable from here - the dialog's
    /// "remember" records what was chosen, and "ask me every time" is a settings
    /// card choice rather than something a close can decide.
    pub fn mode(self) -> ExitMode {
        match self {
            Self::Background => ExitMode::Background,
            Self::KeepRunning => ExitMode::KeepRunning,
            Self::ExitAll => ExitMode::ExitAll,
        }
    }

    /// Whether choosing this leaves the program running.
    pub fn stays_running(self) -> bool {
        matches!(self, Self::Background)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Lang, text};

    /// Every mode survives a round trip through the config file, and the codes are
    /// distinct - a collision would silently change which mode a stored file means.
    #[test]
    fn every_mode_round_trips_through_the_code_it_is_stored_under() {
        let mut codes = Vec::new();
        for mode in ExitMode::ALL {
            assert_eq!(ExitMode::from_code(mode.code()), mode, "{mode:?}");
            codes.push(mode.code());
        }
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ExitMode::ALL.len(), "{codes:?}");
    }

    /// A file from a build that knows a mode this one does not must still start,
    /// and it must start by asking rather than by guessing an answer.
    #[test]
    fn a_mode_this_build_does_not_know_asks() {
        assert_eq!(ExitMode::from_code(""), ExitMode::Ask);
        assert_eq!(ExitMode::from_code("minimize-to-bathtub"), ExitMode::Ask);
        assert_eq!(ExitMode::from_code("  BACKGROUND "), ExitMode::Background);
        assert_eq!(ExitMode::default(), ExitMode::Ask, "asking is the default");
    }

    /// The dialog offers the three exits and the settings card offers the four
    /// modes, and neither list may quietly lose one.
    #[test]
    fn the_mode_and_the_exit_agree_with_each_other() {
        assert_eq!(ExitMode::Ask.choice(), None);
        for mode in ExitMode::ALL {
            match mode.choice() {
                // A remembered mode is exactly one exit, and remembering it back
                // has to land on the mode it came from.
                Some(exit) => assert_eq!(exit.mode(), mode, "{mode:?}"),
                None => assert_eq!(mode, ExitMode::Ask),
            }
        }
        assert_eq!(Exit::Background.mode().choice(), Some(Exit::Background));
    }

    /// What each exit does to the program and to its children. Getting this pair
    /// backwards would make "keep running" stop everything and "exit all" leave
    /// browsers behind, which is the one failure this feature exists to prevent.
    #[test]
    fn only_one_of_the_three_exits_leaves_the_children() {
        // Spelled here rather than as a method: this is the property the three
        // names have to keep, and a helper that answered it would be the thing
        // under test answering its own test.
        let leaves_children = |exit: Exit| matches!(exit, Exit::KeepRunning);

        assert!(Exit::Background.stays_running());
        assert!(!leaves_children(Exit::Background));

        assert!(!Exit::KeepRunning.stays_running());
        assert!(leaves_children(Exit::KeepRunning));

        assert!(!Exit::ExitAll.stays_running());
        assert!(!leaves_children(Exit::ExitAll));
    }

    /// Every mode is named and explained in both languages, and no two modes read
    /// the same - two chips with one sentence would be a switch that cannot be
    /// used.
    #[test]
    fn every_mode_has_its_own_words_in_both_languages() {
        for lang in Lang::ALL {
            let t = text(lang);
            let mut labels = Vec::new();
            let mut notes = Vec::new();
            for mode in ExitMode::ALL {
                assert!(!mode.label(t).trim().is_empty(), "{mode:?}");
                assert!(!mode.note(t).trim().is_empty(), "{mode:?}");
                labels.push(mode.label(t));
                notes.push(mode.note(t));
            }
            labels.sort_unstable();
            labels.dedup();
            notes.sort_unstable();
            notes.dedup();
            assert_eq!(labels.len(), ExitMode::ALL.len(), "{labels:?}");
            assert_eq!(notes.len(), ExitMode::ALL.len(), "{notes:?}");
        }
    }
}
