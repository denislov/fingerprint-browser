//! What this build is.
//!
//! One place, because three surfaces have to agree about it: `--version`, the
//! window's title bar, and the first line of the activity log. A second copy of
//! the version string would be one that eventually disagrees with `Cargo.toml`.

/// The program's name.
///
/// A name rather than a sentence: it is spelled the same in every language the
/// window speaks, like `SOCKS5` and the `FP_BROWSER_*` variables.
pub const NAME: &str = "Fingerprint Browser";

/// The crate version, from the workspace's `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit the binary was built from, injected by `build.rs`.
///
/// `unknown` when there was no git metadata to read, and a trailing `-dirty`
/// when the tree it was built from had uncommitted changes.
pub const COMMIT: &str = env!("FP_BUILD_COMMIT");

/// `Fingerprint Browser 0.1.0 (a1b2c3d)`.
pub fn line() -> String {
    format!("{NAME} {VERSION} ({COMMIT})")
}

/// `linux x86_64 (release)`: what this binary is, and what it can be.
///
/// The platform and the profile both change behaviour a report is about - a
/// debug build has assertions a release build does not - so a report that names
/// the version without them cannot be reproduced.
pub fn platform() -> String {
    format!(
        "{} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        profile()
    )
}

/// Which of cargo's profiles compiled this.
fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// The window's title bar: the name and the version.
///
/// A screenshot then answers the question a report would otherwise have to ask.
pub fn window_title() -> String {
    format!("{NAME} {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_names_the_program_and_the_crate_version() {
        let line = self::line();
        assert!(line.starts_with(NAME), "{line}");
        assert!(line.contains(VERSION), "{line}");
        // The version cargo was told to build, not a second copy of it.
        assert!(line.contains(env!("CARGO_PKG_VERSION")), "{line}");
    }

    /// The injected commit is what makes a bug report reproducible, so an empty
    /// one would be the failure this whole module exists to prevent.
    #[test]
    fn a_commit_is_always_recorded() {
        assert!(!COMMIT.is_empty());
        if COMMIT != "unknown" {
            // `git describe --always` prints an abbreviated hash: hex, and at
            // least the seven characters git's own abbreviation starts at.
            let hash = COMMIT.trim_end_matches("-dirty");
            assert!(hash.len() >= 7, "{COMMIT}");
            assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{COMMIT}");
        }
    }

    #[test]
    fn the_platform_names_the_host_this_binary_runs_on() {
        let platform = self::platform();
        assert!(platform.contains(std::env::consts::OS), "{platform}");
        assert!(platform.contains(std::env::consts::ARCH), "{platform}");
        assert!(
            platform.contains("debug") || platform.contains("release"),
            "{platform}"
        );
    }

    #[test]
    fn the_title_is_the_name_and_the_version_without_the_commit() {
        let title = window_title();
        assert_eq!(title, format!("{NAME} {VERSION}"));
        // The commit belongs in a report, not in a title bar someone has to
        // look at all day.
        assert!(!title.contains(COMMIT), "{title}");
    }
}
