//! Two things the binary needs that only a build can supply.
//!
//! **The commit it was built from**, for `--version`. Two builds of the same
//! crate version differ by exactly the lines between them, and "what version are
//! you running" is the first question every bug report has to answer. Asking
//! `git` here keeps that value out of the source: a checkout reports its own
//! commit, and a source tree with no `.git` (a release tarball, a vendored copy)
//! still builds - it says `unknown` rather than refusing to compile for want of a
//! git repository.
//!
//! **The Windows resource section**, for everything that looks at the
//! executable rather than at the program: Explorer and the taskbar read the icon
//! out of it, and the properties page reads the product name and version. Only a
//! Windows host does this, and a host without a resource compiler gets a warning
//! rather than a failed build - the icon is not worth not being able to compile
//! the program.
//!
//! No `cargo:rerun-if-changed` is emitted for this package's sources, so cargo's
//! default rule applies and this runs again whenever a file here changes. That
//! default is not enough on its own, though: `git commit`, `git checkout` and
//! `git tag` all change what `git describe` answers without touching a file in
//! this package, which is how a committed tree came to report the commit before
//! it - with `-dirty` in it, on a clean working tree. [`watch_git_head`] names the
//! files the answer is actually read from.
//!
//! The icon lives outside this package, so a change to it is picked up on the
//! next source change rather than immediately; that is the cost of keeping the
//! default rule, and it is one a developer regenerating the icon can always pay
//! by touching a source file.

use std::process::Command;

fn main() {
    watch_git_head();

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

    embed_windows_resource();
}

/// Asks cargo to re-run this script when the commit it reports could have
/// changed.
///
/// The answer comes from `git describe`, so the files that matter are the ones
/// git writes it to: the `HEAD` symref, the ref that symref names, and the
/// packed refs a clone or a tag checkout resolves through. The index is
/// deliberately not one of them - every `git status` touches it, and rebuilding
/// this crate on each of those is what keeping the default rule was avoiding.
///
/// A source tree with no `.git` - an unpacked tarball, a vendored copy - watches
/// nothing and keeps the default rule, which is right there: there is no commit
/// to become stale, and naming a path that does not exist would make cargo
/// rebuild on every run for the one answer that is constant.
fn watch_git_head() {
    // Read from the environment rather than with `env!`: cargo sets the manifest
    // directory in the build script's environment, and `env!` at compile time
    // finds nothing there. `embed_windows_resource` exists partly because that
    // mistake compiles on Linux and fails on Windows.
    let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let git = std::path::Path::new(&manifest).join("../..").join(".git");
    if !git.is_dir() {
        return;
    }

    let head = git.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());

    // A symref is what a working checkout has, and a commit moves the file it
    // names rather than `HEAD` itself. A detached `HEAD` - a tag checkout, which
    // is what a release is - holds the commit directly, and the line above
    // covers that case.
    if let Ok(contents) = std::fs::read_to_string(&head)
        && let Some(name) = contents.trim().strip_prefix("ref:")
    {
        println!("cargo:rerun-if-changed={}", git.join(name.trim()).display());
    }

    let packed = git.join("packed-refs");
    if packed.exists() {
        println!("cargo:rerun-if-changed={}", packed.display());
    }
}

/// The icon and the version in the executable's resource section.
///
/// `FileVersion` and `ProductVersion` are filled in from `CARGO_PKG_VERSION` by
/// the crate, so the version a properties page shows is the same one `--version`
/// prints and cannot be a second copy that drifts.
#[cfg(windows)]
fn embed_windows_resource() {
    // Read at run time, not with `env!`: cargo sets the manifest directory in
    // the build script's *environment*, and `env!` at compile time finds nothing
    // there - which is how this fails to build on Windows and nowhere else.
    let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") else {
        println!("cargo:warning=no Windows icon or version resource: no manifest directory");
        return;
    };
    let repository = std::path::Path::new(&manifest).join("../..");
    let icon = repository
        .join("assets/icon.ico")
        .to_string_lossy()
        .to_string();

    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon(&icon)
        .set("ProductName", "Fingerprint Browser")
        .set(
            "FileDescription",
            "Browser profiles for fingerprint-chromium",
        )
        .set("OriginalFilename", "fingerprint-browser.exe")
        .set(
            "LegalCopyright",
            "Copyright (c) 2026 the Fingerprint Browser authors. MIT licensed.",
        );

    // A warning rather than a failure: a machine without a resource compiler can
    // still build and run this program, it just gets an executable with no icon.
    // The release workflow is where that has to be right, and it checks.
    if let Err(error) = resource.compile() {
        println!("cargo:warning=no Windows icon or version resource: {error}");
    }
}

/// Nothing to do: the resource section is a Windows idea.
#[cfg(not(windows))]
fn embed_windows_resource() {}
