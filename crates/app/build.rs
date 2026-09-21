//! The commit this binary was built from, put where `--version` can read it.
//!
//! Two builds of the same crate version differ by exactly the lines between
//! them, and "what version are you running" is the first question every bug
//! report has to answer. Asking `git` here keeps that value out of the source:
//! a checkout reports its own commit, and a source tree with no `.git` (a
//! release tarball, a vendored copy) still builds - it says `unknown` rather
//! than refusing to compile for want of a git repository.
//!
//! No `cargo:rerun-if-changed` is emitted, so cargo's default rule applies and
//! this runs again whenever a file in this package changes - which is the only
//! moment the answer can change. Watching `.git` instead would rerun it every
//! time a git command touched the index, and rebuild the crate for nothing.

use std::process::Command;

fn main() {
    let commit = Command::new("git")
        // `--dirty` marks a build that is not the commit it names, which is the
        // difference between a report that can be reproduced and one that
        // cannot. It reads tracked files only, so `target/` does not mark it.
        .args(["describe", "--always", "--dirty", "--abbrev=9"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=FP_BUILD_COMMIT={commit}");
}
