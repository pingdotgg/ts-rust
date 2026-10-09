use crate::frontend::prelude::*;
use std::time::SystemTime;

// This file ports execute/build/uptodatestatus.go.
// PORT: Go `time.Time` is `Option<SystemTime>`; `None` is the Go zero time,
// as in `vfs::Fs::chtimes`. `Option` orders `None` first, so Go `After`,
// `Before` and `IsZero` are `>`, `<` and `is_none()`.

// Go: build/uptodatestatus.go:9 upToDateStatusType
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum UpToDateStatusType {
    // Errors:

    // config file was not found
    ConfigFileNotFound,
    // found errors during build
    BuildErrors,
    // did not build because upstream project has errors - and we have option to stop build on upstream errors
    UpstreamErrors,

    // Its all good, no work to do
    UpToDate,

    // Pseudo-builds - touch timestamps, no actual build:

    // The project appears out of date because its upstream inputs are newer than its outputs,
    // but all of its outputs are actually newer than the previous identical outputs of its (.d.ts) inputs.
    // This means we can Pseudo-build (just touch timestamps), as if we had actually built this project.
    UpToDateWithUpstreamTypes,
    // The project appears up to date and even though input file changed, its text didnt so just need to update timestamps
    UpToDateWithInputFileText,

    // Needs build:

    // input file is missing
    InputFileMissing,
    // output file is missing
    OutputMissing,
    // input file is newer than output file
    InputFileNewer,
    // build info is out of date as we need to emit some files
    OutOfDateBuildInfoWithPendingEmit,
    // build info indicates that project has errors and they need to be reported
    OutOfDateBuildInfoWithErrors,
    // build info options indicate there is work to do based on changes in options
    OutOfDateOptions,
    // file was root when built but not any more
    OutOfDateRoots,
    // buildInfo.version mismatch with current ts version
    TsVersionOutputOfDate,
    // build because --force was specified
    ForceBuild,

    // solution file
    Solution,
}

// Go: build/uptodatestatus.go:58 inputOutputName
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputOutputName {
    pub input: String,
    pub output: String,
}

// Go: build/uptodatestatus.go:63 fileAndTime
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileAndTime {
    pub file: String,
    pub time: Option<SystemTime>,
}

// Go: build/uptodatestatus.go:68 inputOutputFileAndTime
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputOutputFileAndTime {
    pub input: FileAndTime,
    pub output: FileAndTime,
    pub build_info: String,
}

// Go: build/uptodatestatus.go:73 upstreamErrors
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpstreamErrors {
    pub ref_: String,
    pub ref_has_upstream_errors: bool,
}

// PORT: Go `data any` holds nil, a `string`, or a pointer to one of the
// structs above. This enum lists exactly those cases.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpToDateStatusData {
    #[default]
    Nil,
    String(String),
    InputOutputName(InputOutputName),
    InputOutputFileAndTime(InputOutputFileAndTime),
    UpstreamErrors(UpstreamErrors),
}

// Go: build/uptodatestatus.go:78 upToDateStatus
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpToDateStatus {
    pub kind: UpToDateStatusType,
    pub data: UpToDateStatusData,
}

impl UpToDateStatus {
    // PORT: Go `&upToDateStatus{kind: k}` with nil data.
    pub fn new(kind: UpToDateStatusType) -> Self {
        UpToDateStatus {
            kind,
            data: UpToDateStatusData::Nil,
        }
    }

    // PORT: Go `&upToDateStatus{kind: k, data: d}`.
    pub fn with_data(kind: UpToDateStatusType, data: UpToDateStatusData) -> Self {
        UpToDateStatus { kind, data }
    }

    // Go: build/uptodatestatus.go:83 (*upToDateStatus).isError
    pub fn is_error(&self) -> bool {
        matches!(
            self.kind,
            UpToDateStatusType::ConfigFileNotFound
                | UpToDateStatusType::BuildErrors
                | UpToDateStatusType::UpstreamErrors
        )
    }

    // Go: build/uptodatestatus.go:94 (*upToDateStatus).isPseudoBuild
    pub fn is_pseudo_build(&self) -> bool {
        matches!(
            self.kind,
            UpToDateStatusType::UpToDateWithUpstreamTypes
                | UpToDateStatusType::UpToDateWithInputFileText
        )
    }

    // Go: build/uptodatestatus.go:104 (*upToDateStatus).inputOutputFileAndTime
    pub fn input_output_file_and_time(&self) -> Option<&InputOutputFileAndTime> {
        match &self.data {
            UpToDateStatusData::InputOutputFileAndTime(data) => Some(data),
            _ => None,
        }
    }

    // Go: build/uptodatestatus.go:112 (*upToDateStatus).inputOutputName
    pub fn input_output_name(&self) -> Option<&InputOutputName> {
        match &self.data {
            UpToDateStatusData::InputOutputName(data) => Some(data),
            _ => None,
        }
    }

    // Go: build/uptodatestatus.go:120 (*upToDateStatus).oldestOutputFileName
    pub fn oldest_output_file_name(&self) -> String {
        if !self.is_pseudo_build() && self.kind != UpToDateStatusType::UpToDate {
            panic!("only valid for up to date status of pseudo-build or up to date");
        }

        if let Some(input_output_file_and_time) = self.input_output_file_and_time() {
            return input_output_file_and_time.output.file.clone();
        }
        if let Some(input_output_name) = self.input_output_name() {
            return input_output_name.output.clone();
        }
        // PORT: Go `s.data.(string)` panics on any other data.
        match &self.data {
            UpToDateStatusData::String(s) => s.clone(),
            _ => panic!("interface conversion: data is not string"),
        }
    }

    // Go: build/uptodatestatus.go:134 (*upToDateStatus).upstreamErrors
    pub fn upstream_errors(&self) -> &UpstreamErrors {
        match &self.data {
            UpToDateStatusData::UpstreamErrors(data) => data,
            _ => panic!("interface conversion: data is not *upstreamErrors"),
        }
    }

    // PORT: Go `t.status.data.(string)` in reportUpToDateStatus.
    pub fn data_string(&self) -> &str {
        match &self.data {
            UpToDateStatusData::String(s) => s,
            _ => panic!("interface conversion: data is not string"),
        }
    }
}
