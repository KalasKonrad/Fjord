fn main() {
    slint_build::compile("ui/main.slint").unwrap();

    // Expose a git-based build identifier (matches the PKGBUILD pkgver scheme:
    // r<commit-count>.<short-hash>[-dirty]) so fjord.log can record exactly
    // which build produced it. Without this there's no way to tell a stale
    // log from a fresh one after a forgotten rebuild or uncommitted changes.
    let count = git_output(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "0".into());
    let hash  = git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git_output(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let build_id = format!("r{count}.{hash}{}", if dirty { "-dirty" } else { "" });
    println!("cargo:rustc-env=FJORD_BUILD_ID={build_id}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

fn git_output(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}
