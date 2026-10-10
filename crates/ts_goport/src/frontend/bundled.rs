//! Go: internal/bundled/bundled.go and libs_generated.go.
//!
//! PORT: Go picks the lib files with the `noembed` build tag, and the port
//! with the cargo feature `noembed` (ts_goport forwards it here):
//! - default: `embed.rs` (Go embed.go and embed_generated.go). The libs are
//!   in the binary and are named `bundled:///libs/lib.*.d.ts`. The pinned
//!   reference build (the oracle) is this build.
//! - `noembed`: `noembed.rs` (Go noembed.go). The libs are files next to the
//!   binary, and paths name them. Go's release builds and the shipped npm
//!   packages are this build. `crates/ts_goport/scripts/copy-libs.sh` writes
//!   the lib files.
//!
//! Both builds read the same lib parse and bind snapshots:
//! `bundled_lib_name` gives the base name of a lib file in either form.
//! The wasm build reads none (see `embed::bundled_lib_name`).

use crate::frontend::prelude::*;

#[cfg(not(feature = "noembed"))]
mod embed;
#[cfg(not(feature = "noembed"))]
pub use embed::*;
#[cfg(feature = "noembed")]
mod noembed;
#[cfg(feature = "noembed")]
pub use noembed::*;

/// Bundled lib files with at least this many text bytes get a lib parse and
/// bind snapshot (`binder::lib_snapshot::MIN_TEXT_LEN`): lib.dom (2.3 MB),
/// lib.webworker (0.8 MB) and lib.es5 (0.2 MB). The next largest lib has
/// 40 KB and binds in well under a millisecond, so its section would add
/// binary size for almost no time.
// PORT: not in Go.
pub const SNAPSHOT_TEXT_MIN: usize = 200_000;

/// A 64-bit hash of `bytes` that runs at compile time. It only has to
/// change when the bytes change. The lib snapshots hash their sources with
/// it (`SOURCES_HASH`), and the embed build hashes the text of each
/// snapshot lib with it at compile time (`embedded_text_hash`). It is not
/// xxh3: a const xxh3 of lib.dom takes too long to compile.
// The slice patterns need no bounds check per byte, so the compile-time
// evaluation of the largest file stays far below the rustc step limit.
// PORT: not in Go.
pub const fn const_hash(bytes: &[u8]) -> u64 {
    let mut hash = bytes.len() as u64;
    let mut rest = bytes;
    while let [b0, b1, b2, b3, b4, b5, b6, b7, tail @ ..] = rest {
        hash = mix(hash ^ u64::from_le_bytes([*b0, *b1, *b2, *b3, *b4, *b5, *b6, *b7]));
        rest = tail;
    }
    let mut last = 0u64;
    let mut shift = 0;
    while let [byte, tail @ ..] = rest {
        last |= (*byte as u64) << shift;
        shift += 8;
        rest = tail;
    }
    mix(hash ^ last)
}

/// Multiplies by a 64-bit odd constant and folds the 128-bit product.
// PORT: not in Go.
pub const fn mix(value: u64) -> u64 {
    let product = (value as u128) * 0x9E37_79B9_7F4A_7C15u128;
    (product as u64) ^ ((product >> 64) as u64)
}

// Go: bundled.go:19 Embedded
// Embedded is true if the bundled files are implemented through an embedded FS.
pub const EMBEDDED: bool = EMBEDDED_UNEXPORTED;

// Go: bundled.go:23 WrapFS
// WrapFS returns an FS which redirects embedded paths to the embedded file system.
// If the embedded file system is not available, it returns the original FS.
pub fn wrap_fs_exported(fs: Rc<dyn Fs>) -> Rc<dyn Fs> {
    wrap_fs(fs)
}

// Go: bundled.go:30 LibPath
// LibPath returns the path to the directory containing the bundled lib.d.ts files.
// If embedding is not enabled, this is a path on disk, and must be accessed through
// a real OS filesystem.
pub fn lib_path_exported() -> String {
    lib_path()
}

// PORT: bundled.go:34 bundledSourceDir, testingLibPath and TestingLibPath
// are for Go tests only and are not ported. Tests run on the default build.

// Go: libs_generated.go:7 LibNames
// LibNames is the list of all bundled lib files, sorted by name.
// For the list of libs sorted by load order, use [tsoptions.Libs].
pub static LIB_NAMES: &[&str] = &[
    "lib.d.ts",
    "lib.decorators.d.ts",
    "lib.decorators.legacy.d.ts",
    "lib.dom.asynciterable.d.ts",
    "lib.dom.d.ts",
    "lib.dom.iterable.d.ts",
    "lib.es2015.collection.d.ts",
    "lib.es2015.core.d.ts",
    "lib.es2015.d.ts",
    "lib.es2015.generator.d.ts",
    "lib.es2015.iterable.d.ts",
    "lib.es2015.promise.d.ts",
    "lib.es2015.proxy.d.ts",
    "lib.es2015.reflect.d.ts",
    "lib.es2015.symbol.d.ts",
    "lib.es2015.symbol.wellknown.d.ts",
    "lib.es2016.array.include.d.ts",
    "lib.es2016.d.ts",
    "lib.es2016.full.d.ts",
    "lib.es2016.intl.d.ts",
    "lib.es2017.arraybuffer.d.ts",
    "lib.es2017.d.ts",
    "lib.es2017.date.d.ts",
    "lib.es2017.full.d.ts",
    "lib.es2017.intl.d.ts",
    "lib.es2017.object.d.ts",
    "lib.es2017.sharedmemory.d.ts",
    "lib.es2017.string.d.ts",
    "lib.es2017.typedarrays.d.ts",
    "lib.es2018.asyncgenerator.d.ts",
    "lib.es2018.asynciterable.d.ts",
    "lib.es2018.d.ts",
    "lib.es2018.full.d.ts",
    "lib.es2018.intl.d.ts",
    "lib.es2018.promise.d.ts",
    "lib.es2018.regexp.d.ts",
    "lib.es2019.array.d.ts",
    "lib.es2019.d.ts",
    "lib.es2019.full.d.ts",
    "lib.es2019.intl.d.ts",
    "lib.es2019.object.d.ts",
    "lib.es2019.string.d.ts",
    "lib.es2019.symbol.d.ts",
    "lib.es2020.bigint.d.ts",
    "lib.es2020.d.ts",
    "lib.es2020.date.d.ts",
    "lib.es2020.full.d.ts",
    "lib.es2020.intl.d.ts",
    "lib.es2020.number.d.ts",
    "lib.es2020.promise.d.ts",
    "lib.es2020.sharedmemory.d.ts",
    "lib.es2020.string.d.ts",
    "lib.es2020.symbol.wellknown.d.ts",
    "lib.es2021.d.ts",
    "lib.es2021.full.d.ts",
    "lib.es2021.intl.d.ts",
    "lib.es2021.promise.d.ts",
    "lib.es2021.string.d.ts",
    "lib.es2021.weakref.d.ts",
    "lib.es2022.array.d.ts",
    "lib.es2022.d.ts",
    "lib.es2022.error.d.ts",
    "lib.es2022.full.d.ts",
    "lib.es2022.intl.d.ts",
    "lib.es2022.object.d.ts",
    "lib.es2022.regexp.d.ts",
    "lib.es2022.string.d.ts",
    "lib.es2023.array.d.ts",
    "lib.es2023.collection.d.ts",
    "lib.es2023.d.ts",
    "lib.es2023.full.d.ts",
    "lib.es2023.intl.d.ts",
    "lib.es2024.arraybuffer.d.ts",
    "lib.es2024.collection.d.ts",
    "lib.es2024.d.ts",
    "lib.es2024.full.d.ts",
    "lib.es2024.object.d.ts",
    "lib.es2024.promise.d.ts",
    "lib.es2024.regexp.d.ts",
    "lib.es2024.sharedmemory.d.ts",
    "lib.es2024.string.d.ts",
    "lib.es2025.collection.d.ts",
    "lib.es2025.d.ts",
    "lib.es2025.float16.d.ts",
    "lib.es2025.full.d.ts",
    "lib.es2025.intl.d.ts",
    "lib.es2025.iterator.d.ts",
    "lib.es2025.promise.d.ts",
    "lib.es2025.regexp.d.ts",
    "lib.es2026.array.d.ts",
    "lib.es2026.collection.d.ts",
    "lib.es2026.d.ts",
    "lib.es2026.error.d.ts",
    "lib.es2026.full.d.ts",
    "lib.es2026.iterator.d.ts",
    "lib.es2026.json.d.ts",
    "lib.es2026.math.d.ts",
    "lib.es2026.typedarrays.d.ts",
    "lib.es5.d.ts",
    "lib.es6.d.ts",
    "lib.esnext.d.ts",
    "lib.esnext.date.d.ts",
    "lib.esnext.decorators.d.ts",
    "lib.esnext.disposable.d.ts",
    "lib.esnext.full.d.ts",
    "lib.esnext.intl.d.ts",
    "lib.esnext.modulesource.d.ts",
    "lib.esnext.promise.d.ts",
    "lib.esnext.sharedmemory.d.ts",
    "lib.esnext.temporal.d.ts",
    "lib.scripthost.d.ts",
    "lib.webworker.asynciterable.d.ts",
    "lib.webworker.d.ts",
    "lib.webworker.importscripts.d.ts",
    "lib.webworker.iterable.d.ts",
];
