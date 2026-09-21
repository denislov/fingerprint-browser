//! The command line: the questions answered without opening a window.
//!
//! Two of them - what is this build, and how do I use it - have to be
//! answerable by a script and by someone whose window will not open, so they are
//! decided before anything on disk is read. The third, `--diagnostics`, writes
//! the report a bug report needs and is the only one that touches the
//! installation.
//!
//! Parsing is a pure function over an argument list, so every rule below is a
//! test rather than a manual run - including the rules about what is *refused*,
//! which is the half of a command line that usually goes unexercised until a
//! user finds it.

use crate::text::Text;
use std::path::PathBuf;

/// What this run was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open the window. Also what no arguments at all means.
    Run,
    /// Say what this build is, and stop.
    Version,
    /// Say how to use this program, and stop.
    Help,
    /// Write a report about this installation, and stop.
    Diagnostics { destination: Option<PathBuf> },
}

/// Why an argument list was not one of the above.
///
/// A value rather than a sentence: the language this is reported in comes from
/// the config file, which is read after the arguments and must not have to be
/// read *for* them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Something that is not an option this program has.
    Unknown(String),
    /// An option that needs a value and did not get one.
    MissingValue(String),
    /// An option that only means something alongside another.
    NotApplicable(String),
}

/// The option that names where a report goes.
const OUT: &str = "--out";

/// Reads the arguments after the program's own name.
///
/// Order does not decide between two verbs: `--version --help` prints the help,
/// because the question that only asks for more words is the safe one to answer.
pub fn parse(args: &[String]) -> Result<Command, Refusal> {
    let mut help = false;
    let mut version = false;
    let mut diagnostics = false;
    let mut destination = None;

    let mut index = 0;
    while let Some(argument) = args.get(index).map(String::as_str) {
        index += 1;
        match argument {
            "-h" | "--help" | "help" => help = true,
            "-V" | "--version" | "version" => version = true,
            "--diagnostics" => diagnostics = true,
            _ if argument == OUT || argument.starts_with(&format!("{OUT}=")) => {
                // Both spellings of the same option: `--out /tmp/x` and
                // `--out=/tmp/x`. The second is what a script writes when the
                // path comes from a variable it does not want split.
                let value = match argument.strip_prefix(&format!("{OUT}=")) {
                    Some(inline) => inline.to_string(),
                    None => match args.get(index) {
                        Some(next) => {
                            index += 1;
                            next.clone()
                        }
                        None => return Err(Refusal::MissingValue(OUT.to_string())),
                    },
                };
                if value.trim().is_empty() {
                    return Err(Refusal::MissingValue(OUT.to_string()));
                }
                destination = Some(PathBuf::from(value));
            }
            other => return Err(Refusal::Unknown(other.to_string())),
        }
    }

    if help {
        return Ok(Command::Help);
    }
    if version {
        return Ok(Command::Version);
    }
    if diagnostics {
        return Ok(Command::Diagnostics { destination });
    }
    match destination {
        // A path with nothing to write: refusing says so now, rather than
        // opening a window that quietly ignores half of what was asked.
        Some(_) => Err(Refusal::NotApplicable(OUT.to_string())),
        None => Ok(Command::Run),
    }
}

/// A refusal as the sentence a user reads, in the language in force.
pub fn refusal_message(refusal: &Refusal, t: &Text) -> String {
    match refusal {
        Refusal::Unknown(argument) => t.cli_unknown_argument(argument),
        Refusal::MissingValue(option) => t.cli_missing_value(option),
        Refusal::NotApplicable(option) => t.cli_not_applicable(option),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Command, Refusal> {
        let args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        parse(&args)
    }

    /// The window is what happens when nobody asked for anything else.
    #[test]
    fn no_arguments_opens_the_window() {
        assert_eq!(parse(&[]), Ok(Command::Run));
    }

    #[test]
    fn both_spellings_of_each_question_are_the_same_question() {
        for words in [&["-V"][..], &["--version"][..], &["version"][..]] {
            assert_eq!(parse_words(words), Ok(Command::Version), "{words:?}");
        }
        for words in [&["-h"][..], &["--help"][..], &["help"][..]] {
            assert_eq!(parse_words(words), Ok(Command::Help), "{words:?}");
        }
        assert_eq!(
            parse_words(&["--diagnostics"]),
            Ok(Command::Diagnostics { destination: None })
        );
    }

    /// The one rule that is not about a single argument: the help wins, because
    /// it is the answer that only asks the program to say more.
    #[test]
    fn help_wins_over_version_whatever_the_order() {
        assert_eq!(parse_words(&["--version", "--help"]), Ok(Command::Help));
        assert_eq!(parse_words(&["--help", "--version"]), Ok(Command::Help));
    }

    #[test]
    fn a_report_can_be_aimed_at_a_path_in_either_spelling() {
        let expected = Ok(Command::Diagnostics {
            destination: Some(PathBuf::from("/tmp/report.md")),
        });
        assert_eq!(
            parse_words(&["--diagnostics", "--out", "/tmp/report.md"]),
            expected
        );
        assert_eq!(
            parse_words(&["--diagnostics", "--out=/tmp/report.md"]),
            expected
        );
        // A path with a space in it survives: the value is one argument, not
        // the rest of the line.
        assert_eq!(
            parse_words(&["--out=/tmp/two words.md", "--diagnostics"]),
            Ok(Command::Diagnostics {
                destination: Some(PathBuf::from("/tmp/two words.md")),
            })
        );
    }

    #[test]
    fn an_argument_nobody_knows_is_refused_by_name() {
        assert_eq!(
            parse_words(&["--verbose"]),
            Err(Refusal::Unknown("--verbose".to_string()))
        );
        // A bare word that is not one of the two named verbs is not silently
        // treated as a file to open.
        assert_eq!(
            parse_words(&["profile.json"]),
            Err(Refusal::Unknown("profile.json".to_string()))
        );
    }

    #[test]
    fn an_option_that_needs_a_value_says_which_one() {
        assert_eq!(
            parse_words(&["--diagnostics", "--out"]),
            Err(Refusal::MissingValue(OUT.to_string()))
        );
        // An empty path is the same mistake as no path: there is nowhere to
        // write either way.
        assert_eq!(
            parse_words(&["--diagnostics", "--out="]),
            Err(Refusal::MissingValue(OUT.to_string()))
        );
        assert_eq!(
            parse_words(&["--out", "   ", "--diagnostics"]),
            Err(Refusal::MissingValue(OUT.to_string()))
        );
    }

    /// A path with nothing that writes it is a mistake worth naming, not one to
    /// ignore while the window opens.
    #[test]
    fn a_destination_without_a_report_is_refused() {
        assert_eq!(
            parse_words(&["--out", "/tmp/report.md"]),
            Err(Refusal::NotApplicable(OUT.to_string()))
        );
    }

    #[test]
    fn a_refusal_reads_in_the_language_in_force() {
        use crate::text::{Lang, text};
        let refusal = Refusal::Unknown("--verbose".to_string());

        let english = refusal_message(&refusal, text(Lang::En));
        assert!(english.contains("--verbose"), "{english}");
        let chinese = refusal_message(&refusal, text(Lang::Zh));
        assert!(chinese.contains("--verbose"), "{chinese}");
        assert_ne!(english, chinese, "a refusal is a sentence like any other");
    }
}
