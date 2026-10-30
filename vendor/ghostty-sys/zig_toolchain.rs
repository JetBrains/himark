// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Finds a Zig that ghostty accepts, fetching the pinned release when the machine has none.
//!
//! Ghostty refuses every Zig but one major.minor, and package managers move past it
//! (Homebrew's `zig` is already 0.17). So the build script takes, in order: `ZIG` if set,
//! a `zig` on PATH whose version matches, or the official release archive downloaded once
//! into the user's cache directory and verified against the sha256 ziglang.org publishes.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The release every build uses. Bump it together with [`RELEASES`] when the submodule's
/// `minimum_zig_version` moves; the hashes are in <https://ziglang.org/download/index.json>.
pub const VERSION: &str = "0.16.0";

/// Official release archives by Zig's host name, with their published sha256.
const RELEASES: &[(&str, &str)] = &[
    ("aarch64-macos", "b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489"),
    ("x86_64-macos", "0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7"),
    ("x86_64-linux", "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00"),
    ("aarch64-linux", "ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17"),
    ("x86_64-windows", "68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e"),
    ("aarch64-windows", "aee38316ee4111717900f45dd3130145c39289e105541d737eb8c5ed653c78ef"),
];

/// The Zig executable to build ghostty with; `src` is the ghostty checkout.
pub fn locate(src: &Path) -> PathBuf {
    assert_pin_matches_ghostty(src);
    if let Some(zig) = std::env::var_os("ZIG") {
        return PathBuf::from(zig);
    }
    if version_matches(Path::new("zig")) {
        return PathBuf::from("zig");
    }
    download()
}

fn major_minor(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split(['.', '-', '+']);
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Whether `zig version` reports the pinned major.minor, which is all ghostty checks.
fn version_matches(zig: &Path) -> bool {
    let Ok(output) = Command::new(zig).arg("version").output() else {
        return false;
    };
    output.status.success()
        && major_minor(&String::from_utf8_lossy(&output.stdout)) == major_minor(VERSION)
}

/// The pin follows the submodule: a ghostty bump that moves `minimum_zig_version` fails
/// here until the version and hashes above follow it.
fn assert_pin_matches_ghostty(src: &Path) {
    let zon = std::fs::read_to_string(src.join("build.zig.zon")).expect("ghostty build.zig.zon");
    let required = zon
        .lines()
        .find_map(|l| l.trim().strip_prefix(".minimum_zig_version = \""))
        .and_then(|l| l.split('"').next())
        .expect("build.zig.zon has no minimum_zig_version");
    assert!(
        major_minor(required) == major_minor(VERSION),
        "ghostty-sys pins Zig {VERSION} but vendor/ghostty wants {required}: \
         update VERSION and RELEASES in vendor/ghostty-sys/zig_toolchain.rs"
    );
}

/// Zig's name for the machine running the build script.
fn zig_host() -> String {
    let host = std::env::var("HOST").unwrap();
    let arch = host.split('-').next().unwrap();
    let os = if host.contains("apple-darwin") {
        "macos"
    } else if host.contains("linux") {
        "linux"
    } else if host.contains("windows") {
        "windows"
    } else {
        panic!("ghostty-sys: no Zig {VERSION} release for {host}; install one and set ZIG")
    };
    format!("{arch}-{os}")
}

/// Where downloads live across builds and `cargo clean`: the platform's user cache.
fn cache_dir() -> PathBuf {
    let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches"))
    } else if cfg!(target_os = "windows") {
        env("LOCALAPPDATA")
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(|| PathBuf::from(std::env::var_os("OUT_DIR").unwrap()))
        .join("himark/zig")
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("ghostty-sys: cannot run {what}: {e}"));
    assert!(status.success(), "ghostty-sys: {what} failed: {status}");
}

/// Fetches the pinned release for this host unless the cache already holds it.
fn download() -> PathBuf {
    let host = zig_host();
    let (_, sha256) = RELEASES
        .iter()
        .find(|(h, _)| *h == host)
        .unwrap_or_else(|| panic!("ghostty-sys: no pinned Zig {VERSION} for {host}; set ZIG"));
    let stem = format!("zig-{host}-{VERSION}");
    let archive = format!("{stem}.{}", if cfg!(windows) { "zip" } else { "tar.xz" });
    let root = cache_dir();
    let home = root.join(&stem);
    let exe = home.join(if cfg!(windows) { "zig.exe" } else { "zig" });
    if exe.exists() {
        return exe;
    }

    println!("cargo:warning=ghostty-sys: no Zig {VERSION} on PATH, downloading it to {}", home.display());
    // Unpack beside the destination and rename in, so a crashed or parallel build
    // never leaves a half-extracted toolchain where the next one would find it.
    let staging = root.join(format!(".{stem}.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).unwrap();
    let file = staging.join(&archive);
    let url = format!("https://ziglang.org/download/{VERSION}/{archive}");
    run(
        Command::new("curl").args(["-fsSL", "--retry", "3", "-o"]).arg(&file).arg(&url),
        &format!("curl {url}"),
    );
    let digest = format!("{:x}", Sha256::digest(std::fs::read(&file).unwrap()));
    assert!(
        digest == *sha256,
        "ghostty-sys: {archive} sha256 is {digest}, expected {sha256}; refusing to use it"
    );
    run(
        Command::new("tar").arg("-xf").arg(&file).arg("-C").arg(&staging),
        &format!("tar -xf {archive}"),
    );
    std::fs::remove_file(&file).unwrap();
    if std::fs::rename(staging.join(&stem), &home).is_err() && !exe.exists() {
        panic!("ghostty-sys: cannot place Zig at {}", home.display());
    }
    let _ = std::fs::remove_dir_all(&staging);
    exe
}
