//! Port of execute/incremental/incremental.go (the buildinfo reader) and
//! execute/incremental/host.go (the mtime host).
//!
//! PORT: `ReadBuildInfoProgram` (incremental.go:43) is not here. It builds
//! a `Program` from `buildInfoToSnapshot`, so it lives with `Program` in
//! `program.rs`.

use super::build_info::*;
use crate::frontend::prelude::*;
use std::time::SystemTime;

// Go: incremental/incremental.go:9 BuildInfoReader
pub trait BuildInfoReader {
    fn read_build_info(&self, config: &ParsedCommandLine) -> Option<BuildInfo>;
}

// Go: incremental/incremental.go:15 buildInfoReader
// PORT: Go unexported type behind the `BuildInfoReader` interface.
pub struct BuildInfoReaderImpl {
    host: Rc<dyn CompilerHost>,
}

impl BuildInfoReader for BuildInfoReaderImpl {
    // Go: incremental/incremental.go:19 ReadBuildInfo
    fn read_build_info(&self, config: &ParsedCommandLine) -> Option<BuildInfo> {
        let build_info_file_name = config.get_build_info_file_name();
        if build_info_file_name.is_empty() {
            return None;
        }

        // Read build info file
        let (data, ok) = self.host.fs().read_file(&build_info_file_name);
        if !ok {
            return None;
        }
        parse_build_info(&data)
    }
}

// Go: incremental/incremental.go:31 (json.Unmarshal into BuildInfo)
// PORT: split out of `ReadBuildInfo` so callers with the text in hand (and
// tests) can parse it. Returns `None` on any unmarshal error, like Go.
#[must_use]
pub fn parse_build_info(data: &str) -> Option<BuildInfo> {
    let mut build_info = BuildInfo::default();
    // `data` is the port form of the Go text; Go parses its bytes.
    if json_unmarshal(&go_string_bytes(data), &mut build_info, &[]).is_err() {
        return None;
    }
    Some(build_info)
}

// Go: incremental/program.go:360 (json.Marshal of the BuildInfo)
// PORT: the text that Go writes for a `BuildInfo`. `snapshotToBuildInfo`
// callers use it to write the `.tsbuildinfo` file.
pub fn marshal_build_info(build_info: &BuildInfo) -> Result<String, JsonError> {
    json_marshal(build_info, &[])
}

// Go: incremental/incremental.go:38 NewBuildInfoReader
#[must_use]
pub fn new_build_info_reader(host: Rc<dyn CompilerHost>) -> Rc<dyn BuildInfoReader> {
    Rc::new(BuildInfoReaderImpl { host })
}

// ---------------------------------------------------------------------------
// incremental/host.go
// ---------------------------------------------------------------------------

// Go: incremental/host.go:10 Host
// PORT: Go `time.Time` is `Option<SystemTime>`; `None` is the Go zero time
// (as in `vfs::FileInfo::mod_time`).
pub trait Host {
    fn fs(&self) -> Rc<dyn Fs>;
    fn get_m_time(&self, file_name: &str) -> Option<SystemTime>;
    fn set_m_time(&self, file_name: &str, m_time: Option<SystemTime>) -> Result<(), FsError>;
}

// Go: incremental/host.go:15 host
// PORT: Go unexported type behind the `Host` interface.
pub struct HostImpl {
    host: Rc<dyn CompilerHost>,
}

impl Host for HostImpl {
    // Go: incremental/host.go:22 FS (at 673a5f17d713; removed by ts#64159)
    fn fs(&self) -> Rc<dyn Fs> {
        self.host.fs()
    }

    // Go: incremental/host.go:33 GetMTime
    fn get_m_time(&self, file_name: &str) -> Option<SystemTime> {
        get_m_time(&*self.host, file_name)
    }

    // Go: incremental/host.go:25 SetMTime
    fn set_m_time(&self, file_name: &str, m_time: Option<SystemTime>) -> Result<(), FsError> {
        self.host.fs().chtimes(file_name, None, m_time)
    }
}

// Go: incremental/host.go:29 CreateHost
#[must_use]
pub fn create_host(compiler_host: Rc<dyn CompilerHost>) -> Rc<dyn Host> {
    Rc::new(HostImpl {
        host: compiler_host,
    })
}

// Go: incremental/host.go:33 GetMTime
#[must_use]
pub fn get_m_time(host: &dyn CompilerHost, file_name: &str) -> Option<SystemTime> {
    let stat = host.fs().stat(file_name);
    let mut m_time = None;
    if let Some(stat) = stat {
        m_time = stat.mod_time();
    }
    m_time
}
