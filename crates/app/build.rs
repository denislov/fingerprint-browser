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
//! No `cargo:rerun-if-changed` is emitted, so cargo's default rule applies and
//! this runs again whenever a file in this package changes - which is the only
//! moment the commit can change. Watching `.git` instead would rerun it every
//! time a git command touched the index, and rebuild the crate for nothing. The
//! icon lives outside this package, so a change to it is picked up on the next
//! source change rather than immediately; that is the cost of keeping the default
//! rule, and it is one a developer regenerating the icon can always pay by
//! touching a source file.

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

    embed_windows_resource();
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
