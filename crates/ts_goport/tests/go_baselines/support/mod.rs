pub mod baseline;
pub mod child;
pub mod contentmappertest;
pub mod fsbaselineutil;
pub mod harnessutil;
pub mod iovfs;
pub mod mock_watch_backend;
pub mod patience;
pub mod readable_build_info;
pub mod runner;
mod smoke;
pub mod stringtestutil;
pub mod test_sys;
pub mod vfstest;
mod vfstest_test;

/// Go `filepath.EvalSymlinks`: the real path of `path`.
// PORT: `std::fs::canonicalize` gives a verbatim path on Windows
// (`\\?\C:\...`), which Go does not, and which takes no `/` separators.
pub fn eval_symlinks(path: impl AsRef<std::path::Path>) -> std::io::Result<std::path::PathBuf> {
    let real = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    if let Some(rest) = real.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && !rest.starts_with(r"UNC\")
    {
        return Ok(std::path::PathBuf::from(rest));
    }
    Ok(real)
}
