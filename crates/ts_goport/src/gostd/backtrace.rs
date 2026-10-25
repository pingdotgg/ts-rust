//! The text of a Rust backtrace where Go prints a goroutine stack: the panic
//! answers of the API and IPC (Go `debug.Stack()`), the LSP log
//! (`core::go_recovered_stack`) and `GOPORT_TRACE` output.
//!
//! PORT: the shipped tsc has no symbol table (release.yml strips it). There
//! std names no frame, and its short text is only `   N: <unknown>`. `text`
//! then adds what the unstripped tsc needs to name each frame later
//! (PORTING.md "Release builds", "Panic frames of a stripped tsc").

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::io::Read as _;
use std::sync::OnceLock;

const UNKNOWN: &str = "<unknown>";
/// The bytes of the running binary that `binary_id` reads. The ELF program
/// headers and notes and the Mach-O load commands are at the start.
const HEAD_BYTES: u64 = 64 * 1024;
const PT_NOTE: u32 = 4;
const NT_GNU_BUILD_ID: u32 = 3;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_UUID: u32 = 0x1b;

/// std's short text of `backtrace` (`{backtrace}`), unchanged when every
/// frame has a name. Otherwise a frame without a name prints as in std's
/// full text, `   N: 0x6c5d2a - <unknown>` (its return address), and the
/// text starts with one line for the binary:
/// `build-id <hex>; gostd::backtrace::text at 0x<address>`. The id is also
/// in the unstripped binary. The address of this function gives the load
/// offset (0 for the static Linux tsc).
pub fn text(backtrace: &Backtrace) -> String {
    let short = backtrace.to_string();
    if !short
        .lines()
        .any(|line| matches!(frame_line(line), Some((_, UNKNOWN))))
    {
        return short;
    }
    let anchor = text as *const () as usize;
    let id = binary_id().map(|id| format!("{id}; ")).unwrap_or_default();
    let mut out = format!("{id}gostd::backtrace::text at {anchor:#x}\n");
    out.push_str(&with_addresses(&short, &format!("{backtrace:#}")).unwrap_or(short));
    out
}

/// `short` (std's short text) with the address of each `<unknown>` frame,
/// which `full` (std's full text of the same backtrace) has. `full` lists
/// std's own capture frames first, so its indexes are higher by their
/// count. Its last frame that is not null is the last frame of `short`
/// (the short text skips null frames).
fn with_addresses(short: &str, full: &str) -> Option<String> {
    let addresses: Vec<(usize, &str)> = full
        .lines()
        .filter_map(|line| {
            let (index, rest) = frame_line(line)?;
            Some((index, rest.trim_start().split_once(" - ")?.0))
        })
        .collect();
    let last_full = addresses
        .iter()
        .rev()
        .find(|(_, address)| *address != "0x0")?
        .0;
    let last_short = short.lines().filter_map(frame_line).last()?.0;
    let skipped = last_full.checked_sub(last_short)?;
    let mut out = String::with_capacity(short.len() * 2);
    for line in short.lines() {
        if let Some((index, UNKNOWN)) = frame_line(line)
            && let Some((_, address)) = addresses.iter().find(|(i, _)| *i == index + skipped)
        {
            let _ = writeln!(out, "{index:4}: {address} - {UNKNOWN}");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Some(out)
}

/// The index and the rest of a frame line of std's text (`{index:4}: ...`).
fn frame_line(line: &str) -> Option<(usize, &str)> {
    let (index, rest) = line.trim_start().split_once(": ")?;
    Some((index.parse().ok()?, rest))
}

/// The id that the running binary shares with its unstripped copy, read once:
/// `build-id <hex>` for ELF (the GNU build-id note, as `readelf -n` prints
/// it), `uuid <UUID>` for Mach-O (LC_UUID, as `dwarfdump --uuid` prints
/// it). None for a PE binary (Windows) and when the file cannot be read.
fn binary_id() -> Option<&'static str> {
    static ID: OnceLock<Option<String>> = OnceLock::new();
    ID.get_or_init(|| {
        // Linux: /proc/self/exe is the running file, also after an update
        // replaced the file on disk.
        let path = if cfg!(target_os = "linux") {
            "/proc/self/exe".into()
        } else {
            std::env::current_exe().ok()?
        };
        let mut head = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(HEAD_BYTES)
            .read_to_end(&mut head)
            .ok()?;
        elf_build_id(&head).or_else(|| macho_uuid(&head))
    })
    .as_deref()
}

/// `build-id <hex>`: the GNU build-id note of a 64-bit little-endian ELF
/// file whose first bytes are `head`.
fn elf_build_id(head: &[u8]) -> Option<String> {
    // ELFCLASS64, ELFDATA2LSB.
    if head.get(..6)? != b"\x7fELF\x02\x01" {
        return None;
    }
    let phoff = usize::try_from(u64::from_le_bytes(le(head, 0x20)?)).ok()?;
    let phentsize = usize::from(u16::from_le_bytes(le(head, 0x36)?));
    let phnum = usize::from(u16::from_le_bytes(le(head, 0x38)?));
    (0..phnum).find_map(|i| {
        let ph = phoff.checked_add(i.checked_mul(phentsize)?)?;
        if u32::from_le_bytes(le(head, ph)?) != PT_NOTE {
            return None;
        }
        let offset = usize::try_from(u64::from_le_bytes(le(head, ph + 8)?)).ok()?;
        let size = usize::try_from(u64::from_le_bytes(le(head, ph + 32)?)).ok()?;
        let align = if u64::from_le_bytes(le(head, ph + 48)?) == 8 {
            8
        } else {
            4
        };
        let mut notes = head.get(offset..offset.checked_add(size)?)?;
        // Each note: name size, desc size, type, then the name and the desc,
        // each padded to `align`.
        while notes.len() >= 12 {
            let name_size = usize::try_from(u32::from_le_bytes(le(notes, 0)?)).ok()?;
            let desc_size = usize::try_from(u32::from_le_bytes(le(notes, 4)?)).ok()?;
            let desc_start = 12usize.checked_add(name_size.checked_next_multiple_of(align)?)?;
            let desc = notes.get(desc_start..desc_start.checked_add(desc_size)?)?;
            if u32::from_le_bytes(le(notes, 8)?) == NT_GNU_BUILD_ID
                && notes.get(12..12 + name_size)? == b"GNU\0"
            {
                return Some(format!("build-id {}", hex(desc)));
            }
            let next = desc_start.checked_add(desc_size.checked_next_multiple_of(align)?)?;
            notes = notes.get(next.min(notes.len())..)?;
        }
        None
    })
}

/// `uuid <UUID>`: the LC_UUID of a 64-bit little-endian Mach-O file (not a
/// universal one) whose first bytes are `head`.
fn macho_uuid(head: &[u8]) -> Option<String> {
    if u32::from_le_bytes(le(head, 0)?) != MH_MAGIC_64 {
        return None;
    }
    let commands = u32::from_le_bytes(le(head, 16)?);
    // The load commands follow the 32-byte header: command, size, data.
    let mut at = 32usize;
    for _ in 0..commands {
        if u32::from_le_bytes(le(head, at)?) == LC_UUID {
            let uuid = hex(head.get(at + 8..at + 24)?).to_uppercase();
            let (a, rest) = uuid.split_at(8);
            let (b, rest) = rest.split_at(4);
            let (c, rest) = rest.split_at(4);
            let (d, e) = rest.split_at(4);
            return Some(format!("uuid {a}-{b}-{c}-{d}-{e}"));
        }
        let size = usize::try_from(u32::from_le_bytes(le(head, at + 4)?)).ok()?;
        at = at.checked_add(size.max(8))?;
    }
    None
}

/// The `N` bytes of `bytes` at `at`, for `from_le_bytes`.
fn le<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts of one backtrace as std 1.95 prints them: two capture
    /// frames, a named frame, two frames without a name and a null frame.
    const SHORT: &str = "   0: ts_goport::api::Session::handle_batch_request
   1: <unknown>
   2: <unknown>
";
    const FULL: &str =
        "   0:           0x6f1a10 - std::backtrace_rs::backtrace::libunwind::trace::h1
   1:           0x6f1a80 - std::backtrace::Backtrace::create::h2
   2:           0x4c2b31 - ts_goport::api::Session::handle_batch_request::h3
   3:           0x4c1f07 - <unknown>
   4:           0x401c2e - <unknown>
   5:                0x0 - <unknown>
";

    #[test]
    fn unknown_frames_get_their_address() {
        assert_eq!(
            with_addresses(SHORT, FULL).as_deref(),
            Some(
                "   0: ts_goport::api::Session::handle_batch_request
   1: 0x4c1f07 - <unknown>
   2: 0x401c2e - <unknown>
"
            )
        );
    }

    /// std's real texts line up: with every name of the short text taken
    /// out (a stripped binary), each frame gets the address that the full
    /// text prints beside its name.
    #[test]
    fn std_texts_line_up() {
        let backtrace = Backtrace::force_capture();
        let short = backtrace.to_string();
        let full = format!("{backtrace:#}");
        let stripped: String = short
            .lines()
            .map(|line| match frame_line(line) {
                Some((index, _)) => format!("{index:4}: {UNKNOWN}\n"),
                None => format!("{line}\n"),
            })
            .collect();
        let out = with_addresses(&stripped, &full).expect("addresses");
        let names: Vec<_> = short.lines().filter_map(frame_line).collect();
        let frames: Vec<_> = out.lines().filter_map(frame_line).collect();
        assert_eq!(names.len(), frames.len());
        assert!(
            names
                .iter()
                .any(|(_, name)| name.contains("std_texts_line_up"))
        );
        for ((index, name), (out_index, rest)) in names.iter().zip(&frames) {
            assert_eq!(index, out_index);
            let (address, _) = rest.split_once(" - ").expect("an address");
            let named = full
                .lines()
                .filter_map(|line| line.split_once(&format!(" {address} - ")))
                .any(|(_, full_name)| without_hashes(full_name).starts_with(name));
            assert!(
                named,
                "frame {index} {name}: {address} has another name in\n{full}"
            );
        }
    }

    /// A name of the full text without its hashes: v0 crate hashes
    /// (`std[7714dba5b6ef3665]::`), which rustc 1.95 prints. A legacy
    /// `::h0123...` suffix stays (the short name is then a prefix).
    fn without_hashes(name: &str) -> String {
        let mut out = String::new();
        let mut rest = name;
        while let Some((head, tail)) = rest.split_once('[') {
            out.push_str(head);
            match tail.split_once(']') {
                Some((hash, after))
                    if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) =>
                {
                    rest = after;
                }
                _ => {
                    out.push('[');
                    rest = tail;
                }
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn binary_ids() {
        let id = [0xab; 20];
        // An ELF header with one PT_NOTE program header at 64 and its note
        // at 120.
        let mut elf = vec![0u8; 120];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
        elf[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
        elf[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&PT_NOTE.to_le_bytes());
        elf[72..80].copy_from_slice(&120u64.to_le_bytes());
        elf[96..104].copy_from_slice(&36u64.to_le_bytes());
        elf[112..120].copy_from_slice(&4u64.to_le_bytes());
        for word in [4, 20, NT_GNU_BUILD_ID] {
            elf.extend_from_slice(&word.to_le_bytes());
        }
        elf.extend_from_slice(b"GNU\0");
        elf.extend_from_slice(&id);
        assert_eq!(
            elf_build_id(&elf).as_deref(),
            Some(format!("build-id {}", "ab".repeat(20)).as_str())
        );

        // A Mach-O header with two load commands: a segment, then LC_UUID.
        let mut macho = vec![0u8; 32];
        macho[..4].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
        macho[16..20].copy_from_slice(&2u32.to_le_bytes());
        for word in [0x19, 16, 0, 0, LC_UUID, 24] {
            macho.extend_from_slice(&u32::to_le_bytes(word));
        }
        macho.extend_from_slice(&id[..16]);
        assert_eq!(
            macho_uuid(&macho).as_deref(),
            Some("uuid ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB")
        );
        assert_eq!(elf_build_id(&macho), None);
        assert_eq!(macho_uuid(&elf), None);
    }
}
