//! Go: execute/tsc/statistics.go, the `--diagnostics` and
//! `--extendedDiagnostics` table.

use std::time::Duration;

use crate::prelude::*;

use super::compile::CompileTimes;
// PORT: testing (`report_to`)
use super::compile::{CommandLineTesting, Writer, write_str};

struct TableRow {
    name: String,
    value: String,
}

#[derive(Default)]
struct Table {
    rows: Vec<TableRow>,
}

impl Table {
    // Go: execute/tsc/statistics.go:24 add
    // PORT: Go `add` takes `any` and formats a `time.Duration` with
    // `formatDuration`. Durations use `add_duration` here.
    fn add(&mut self, name: &str, value: impl std::fmt::Display) {
        self.rows.push(TableRow {
            name: name.to_string(),
            value: value.to_string(),
        });
    }

    fn add_duration(&mut self, name: &str, value: Duration) {
        self.add(name, format_duration(value));
    }

    // Go: execute/tsc/statistics.go:31 print
    // One write for each row, as Go's `fmt.Fprintf`.
    fn print(&self, w: &Writer) {
        let mut name_width = 0;
        let mut value_width = 0;
        for r in &self.rows {
            name_width = name_width.max(r.name.len());
            value_width = value_width.max(r.value.len());
        }

        for r in &self.rows {
            write_str(
                w,
                &format!(
                    "{:<name_width$} {:>value_width$}\n",
                    format!("{}:", r.name),
                    r.value,
                    name_width = name_width + 1,
                ),
            );
        }
    }
}

// Go: execute/tsc/statistics.go:44 formatDuration
fn format_duration(d: Duration) -> String {
    format!("{:.3}s", d.as_secs_f64())
}

// PORT: Go execute/tsc/statistics.go:48 `identifierCount` has no caller
// (`statisticsFromProgram` uses `Program.IdentifierCount`), so it is not
// ported.

// Go: execute/tsc/statistics.go:56 Statistics
#[derive(Clone, Debug, Default)]
pub struct Statistics {
    is_aggregate: bool,
    pub projects: i32,
    pub projects_built: i32,
    pub timestamp_updates: i32,
    files: i32,
    lines: i32,
    // PORT: Go `int`, so `i64` (see `program::identifier_count`).
    identifiers: i64,
    symbols: i32,
    types: i32,
    instantiations: i32,
    memory_used: u64,
    memory_allocs: u64,
    compile_times: Option<CompileTimes>,
}

/// Go `runtime.MemStats`, the two fields that `statisticsFromProgram` reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemStats {
    /// Go `Alloc`: bytes of live heap objects.
    pub alloc: u64,
    /// Go `Mallocs`: count of heap objects allocated.
    pub mallocs: u64,
}

/// Go `runtime.GC(); runtime.GC(); runtime.ReadMemStats(&memStats)`.
// PORT: Rust has no garbage collector. With jemalloc as the allocator (the
// default `jemalloc` feature: every release build but Windows), `alloc` is
// jemalloc's `stats.allocated`, the bytes that the process allocated and
// did not free (Go `Alloc`), and `mallocs` the allocation requests of all
// arenas (Go `Mallocs`; jemalloc adds the requests of a thread cache when
// it flushes the cache). Elsewhere `alloc` is the resident set size
// (`VmRSS` in /proc/self/status, 0 where that file does not exist), and
// `mallocs` is 0: the crate forbids the `unsafe` of a counting global
// allocator. These are process values that never match Go anyway.
#[must_use]
pub fn read_mem_stats() -> MemStats {
    jemalloc_mem_stats().unwrap_or_else(|| MemStats {
        alloc: resident_bytes(),
        mallocs: 0,
    })
}

/// jemalloc's statistics (see `read_mem_stats`), through the safe `Access`
/// API of tikv-jemalloc-ctl. None when jemalloc keeps no statistics (built
/// without `stats`) or is not the allocator (nothing allocated).
#[cfg(all(feature = "jemalloc", not(windows)))]
fn jemalloc_mem_stats() -> Option<MemStats> {
    use tikv_jemalloc_ctl::{Access, AsName, epoch};
    // The statistics are a snapshot that a new epoch takes.
    epoch::advance().ok()?;
    let alloc: usize = b"stats.allocated\0".name().read().ok()?;
    // Arena 4096 (`MALLCTL_ARENAS_ALL`) sums all arenas.
    let requests = |name: &[u8]| Access::<u64>::read(name.name()).ok();
    let mallocs = requests(b"stats.arenas.4096.small.nrequests\0")?
        + requests(b"stats.arenas.4096.large.nrequests\0")?;
    Some(MemStats {
        alloc: u64::try_from(alloc).ok()?,
        mallocs,
    })
    .filter(|stats| stats.alloc != 0)
}

#[cfg(not(all(feature = "jemalloc", not(windows))))]
fn jemalloc_mem_stats() -> Option<MemStats> {
    None
}

/// The resident set size of this process in bytes (`VmRSS` in
/// /proc/self/status), 0 where that file does not exist.
fn resident_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                let kib = line.strip_prefix("VmRSS:")?.trim().strip_suffix("kB")?;
                kib.trim().parse::<u64>().ok()
            })
        })
        .map_or(0, |kib| kib * 1024)
}

// Go: execute/tsc/statistics.go:72 statisticsFromProgram
// PORT: Go reads the program and the compile times from `EmitInput`. The
// program is the current program (`prog()`) here.
#[must_use]
pub fn statistics_from_program(compile_times: &CompileTimes, mem_stats: &MemStats) -> Statistics {
    Statistics {
        files: source_files().len() as i32,
        lines: crate::program::line_count(),
        identifiers: crate::program::identifier_count(),
        symbols: crate::program::symbol_count(),
        types: crate::program::type_count(),
        instantiations: crate::program::instantiation_count(),
        memory_used: mem_stats.alloc,
        memory_allocs: mem_stats.mallocs,
        compile_times: Some(compile_times.clone()),
        ..Statistics::default()
    }
}

impl Statistics {
    // Go: execute/tsc/statistics.go:86 Report, the table part.
    // PORT: `report_to` is Go `Report` with the `CommandLineTesting` hooks.
    fn report(&self, w: &Writer) {
        let mut table = Table::default();
        let mut prefix = "";

        if self.is_aggregate {
            prefix = "Aggregate ";
            table.add("Projects in scope", self.projects);
            table.add("Projects built", self.projects_built);
            table.add("Timestamps only updates", self.timestamp_updates);
        }
        table.add(&format!("{prefix}Files"), self.files);
        table.add(&format!("{prefix}Lines"), self.lines);
        table.add(&format!("{prefix}Identifiers"), self.identifiers);
        table.add(&format!("{prefix}Symbols"), self.symbols);
        table.add(&format!("{prefix}Types"), self.types);
        table.add(&format!("{prefix}Instantiations"), self.instantiations);
        table.add(
            &format!("{prefix}Memory used"),
            format!("{}K", self.memory_used / 1024),
        );
        table.add(&format!("{prefix}Memory allocs"), self.memory_allocs);
        // Go dereferences the pointer; a nil one panics there.
        let compile_times = self
            .compile_times
            .as_ref()
            .expect("statistics without compile times");
        if !compile_times.config_time.is_zero() {
            table.add_duration(&format!("{prefix}Config time"), compile_times.config_time);
        }
        if !compile_times.build_info_read_time.is_zero() {
            table.add_duration(
                &format!("{prefix}BuildInfo read time"),
                compile_times.build_info_read_time,
            );
        }
        table.add_duration(&format!("{prefix}Parse time"), compile_times.parse_time);
        if !compile_times.bind_time.is_zero() {
            table.add_duration(&format!("{prefix}Bind time"), compile_times.bind_time);
        }
        if !compile_times.check_time.is_zero() {
            table.add_duration(&format!("{prefix}Check time"), compile_times.check_time);
        }
        if !compile_times.emit_time.is_zero() {
            table.add_duration(&format!("{prefix}Emit time"), compile_times.emit_time);
        }
        if !compile_times.changes_compute_time.is_zero() {
            table.add_duration(
                &format!("{prefix}Changes compute time"),
                compile_times.changes_compute_time,
            );
        }
        self.add_content_mapper_statistics(&mut table, prefix);
        table.add_duration(&format!("{prefix}Total time"), compile_times.total_time);
        table.print(w);
    }

    // Go: execute/tsc/statistics.go:132 addContentMapperStatistics (tsgo#4712)
    // PORT: Go `slices.Sorted(maps.Keys(timings.Mappers))` sorts the
    // identities by their bytes, as `str` ordering does.
    fn add_content_mapper_statistics(&self, table: &mut Table, prefix: &str) {
        let timings = &self
            .compile_times
            .as_ref()
            .expect("statistics without compile times")
            .content_mapper_times;
        if !timings.request_wait.is_zero() {
            table.add_duration(
                &format!("{prefix}Content mapper request wait time"),
                timings.request_wait,
            );
        }
        let mut identities: Vec<&String> = timings.mappers.keys().collect();
        crate::gostd::slices::stable_sort_by(&mut identities, Ord::cmp);
        for identity in identities {
            let mapper = &timings.mappers[identity];
            let initialization_count = mapper.spawn.count;
            if initialization_count != 0 {
                table.add_duration(
                    &format!("{prefix}{identity} initialization time"),
                    mapper.spawn.duration + mapper.initialize.duration,
                );
            }
            if mapper.transform.count != 0 {
                table.add_duration(
                    &format!("{prefix}{identity} transform time"),
                    mapper.transform.duration,
                );
            }
            if mapper.open_project.count != 0 {
                table.add_duration(
                    &format!("{prefix}{identity} openProject time"),
                    mapper.open_project.duration,
                );
            }
            if mapper.close_project.count != 0 {
                table.add_duration(
                    &format!("{prefix}{identity} closeProject time"),
                    mapper.close_project.duration,
                );
            }
        }
    }

    // Go: execute/tsc/statistics.go:86 Report
    // PORT: testing. Go `defer testing.OnStatisticsEnd(w)` runs after the
    // table is written.
    pub fn report_to(&self, w: &Writer, testing: Option<Rc<dyn CommandLineTesting>>) {
        if let Some(testing) = &testing {
            testing.on_statistics_start(w);
        }
        // PORT: one report (`stdio::keep_writes`).
        let report = super::stdio::keep_writes();
        self.report(w);
        drop(report);
        if let Some(testing) = &testing {
            testing.on_statistics_end(w);
        }
    }

    // Go: execute/tsc/statistics.go:155 Aggregate
    pub fn aggregate(&mut self, stat: &Statistics) {
        self.is_aggregate = true;
        let compile_times = self.compile_times.get_or_insert_with(CompileTimes::default);
        // Aggregate statistics
        self.files += stat.files;
        self.lines += stat.lines;
        // Go `int` wraps (see `program::identifier_count`).
        self.identifiers = self.identifiers.wrapping_add(stat.identifiers);
        self.symbols += stat.symbols;
        self.types += stat.types;
        self.instantiations += stat.instantiations;
        self.memory_used += stat.memory_used;
        self.memory_allocs += stat.memory_allocs;
        let stat_times = stat
            .compile_times
            .as_ref()
            .expect("statistics without compile times");
        compile_times.config_time += stat_times.config_time;
        compile_times.build_info_read_time += stat_times.build_info_read_time;
        compile_times.parse_time += stat_times.parse_time;
        compile_times.bind_time += stat_times.bind_time;
        compile_times.check_time += stat_times.check_time;
        compile_times.emit_time += stat_times.emit_time;
        compile_times.changes_compute_time += stat_times.changes_compute_time;
    }

    // Go: execute/tsc/statistics.go:178 SetTotalTime
    pub fn set_total_time(&mut self, total_time: Duration) {
        self.compile_times
            .get_or_insert_with(CompileTimes::default)
            .total_time = total_time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go pads names left-aligned to the widest name plus the colon, and
    // values right-aligned to the widest value.
    #[test]
    fn table_print_matches_go_widths() {
        let mut table = Table::default();
        table.add("Files", 750);
        table.add("Memory used", "165135K");
        table.add_duration("Changes compute time", Duration::from_micros(33_400));
        let out = Rc::new(RefCell::new(Vec::<u8>::new()));
        table.print(&(out.clone() as Writer));
        assert_eq!(
            String::from_utf8(out.take()).unwrap(),
            "Files:                    750\n\
             Memory used:          165135K\n\
             Changes compute time:  0.033s\n"
        );
    }

    // Go `table.print` writes each row with one `fmt.Fprintf` on the
    // unbuffered `os.Stdout`. strace of the pin N oracle with
    // `--diagnostics` on a one-file project: these 14 writes of fd 1. The
    // system writer writes each write of the table at once
    // (`stdio::CliStdout`), so `report` must give one write per row.
    // The input is the raw statistics, not the expected text: the test
    // also checks the rows that `report` picks, `Memory used` in KiB
    // (bytes / 1024, truncated), the rounding of each duration to ms, and
    // that a duration under 0.5 ms that is not zero (`Emit time`) keeps
    // its row.
    #[test]
    fn table_print_writes_go_pieces() {
        let go_writes = [
            "Files:              91\n",
            "Lines:           58774\n",
            "Identifiers:     49870\n",
            "Symbols:         59387\n",
            "Types:           34675\n",
            "Instantiations:  33953\n",
            "Memory used:    59443K\n",
            "Memory allocs:  271214\n",
            "Config time:    0.004s\n",
            "Parse time:     0.077s\n",
            "Bind time:      0.041s\n",
            "Check time:     0.387s\n",
            "Emit time:      0.000s\n",
            "Total time:     0.514s\n",
        ];
        let statistics = Statistics {
            files: 91,
            lines: 58774,
            identifiers: 49870,
            symbols: 59387,
            types: 34675,
            instantiations: 33953,
            memory_used: 59443 * 1024 + 1000,
            memory_allocs: 271_214,
            compile_times: Some(CompileTimes {
                config_time: Duration::from_micros(4_400),
                parse_time: Duration::from_micros(76_600),
                bind_time: Duration::from_micros(41_200),
                check_time: Duration::from_micros(386_700),
                emit_time: Duration::from_micros(200),
                total_time: Duration::from_micros(514_300),
                ..CompileTimes::default()
            }),
            ..Statistics::default()
        };
        /// Keeps each write as one piece, an empty one too, as the system
        /// writer writes each one (`GoOutput`).
        #[derive(Default)]
        struct Writes(Vec<String>);
        impl std::io::Write for Writes {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.write_all(buf)?;
                Ok(buf.len())
            }
            fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
                self.0.push(String::from_utf8(buf.to_vec()).unwrap());
                Ok(())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let writes = Rc::new(RefCell::new(Writes::default()));
        statistics.report(&(writes.clone() as Writer));
        assert_eq!(writes.take().0, go_writes);
    }

    // Go counts identifiers in `int`, 64 bits. The JSDoc reparse of one
    // file can make more than 2^31 of them: the pin 3e9236361fa9 oracle
    // prints this table for `--extendedDiagnostics` on the jsdocmem1
    // skeptic input `sk-bun0327-34000` (an i32 count printed
    // `-1982887150`). The sum of `aggregate` and the rows of `report` keep
    // the count.
    #[test]
    fn identifiers_past_i32_max_match_go() {
        let times = |parse: u64, bind: u64, check: u64, total: u64| CompileTimes {
            config_time: Duration::from_micros(100),
            parse_time: Duration::from_millis(parse),
            bind_time: Duration::from_millis(bind),
            check_time: Duration::from_millis(check),
            emit_time: Duration::from_micros(100),
            total_time: Duration::from_millis(total),
            ..CompileTimes::default()
        };
        let mut sum = Statistics::default();
        for identifiers in [1_200_000_000, 1_112_080_146] {
            sum.aggregate(&Statistics {
                identifiers,
                compile_times: Some(times(0, 0, 0, 0)),
                ..Statistics::default()
            });
        }
        assert_eq!(sum.identifiers, 2_312_080_146);

        let statistics = Statistics {
            files: 96,
            lines: 14353,
            identifiers: sum.identifiers,
            symbols: 8580,
            types: 343,
            instantiations: 0,
            memory_used: 1_221_248 * 1024,
            memory_allocs: 1_470_897_153,
            compile_times: Some(times(231_143, 1, 17, 231_162)),
            ..Statistics::default()
        };
        let out = Rc::new(RefCell::new(Vec::<u8>::new()));
        statistics.report(&(out.clone() as Writer));
        assert_eq!(
            String::from_utf8(out.take()).unwrap(),
            "Files:                  96\n\
             Lines:               14353\n\
             Identifiers:    2312080146\n\
             Symbols:              8580\n\
             Types:                 343\n\
             Instantiations:          0\n\
             Memory used:      1221248K\n\
             Memory allocs:  1470897153\n\
             Config time:        0.000s\n\
             Parse time:       231.143s\n\
             Bind time:          0.001s\n\
             Check time:         0.017s\n\
             Emit time:          0.000s\n\
             Total time:       231.162s\n"
        );
    }

    // The content mapper rows (tsgo#4712): identities in sorted order, and
    // only the operations that ran. Go's map order is random, so the sort
    // is what makes the rows stable.
    #[test]
    fn content_mapper_rows_are_sorted_and_skip_unused_operations() {
        use crate::contentmapper::{MapperTimings, OperationTiming};
        let ran = |millis| OperationTiming {
            count: 1,
            duration: Duration::from_millis(millis),
        };
        let mut compile_times = CompileTimes::default();
        compile_times.content_mapper_times.request_wait = Duration::from_millis(5);
        compile_times.content_mapper_times.mappers.insert(
            "z-mapper".to_string(),
            MapperTimings {
                transform: ran(20),
                ..MapperTimings::default()
            },
        );
        compile_times.content_mapper_times.mappers.insert(
            "a-mapper".to_string(),
            MapperTimings {
                spawn: ran(1),
                initialize: ran(2),
                open_project: ran(3),
                ..MapperTimings::default()
            },
        );
        let statistics = Statistics {
            compile_times: Some(compile_times),
            ..Statistics::default()
        };
        let mut table = Table::default();
        statistics.add_content_mapper_statistics(&mut table, "");
        let rows: Vec<(&str, &str)> = table
            .rows
            .iter()
            .map(|row| (row.name.as_str(), row.value.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Content mapper request wait time", "0.005s"),
                ("a-mapper initialization time", "0.003s"),
                ("a-mapper openProject time", "0.003s"),
                ("z-mapper transform time", "0.020s"),
            ]
        );
    }
}
