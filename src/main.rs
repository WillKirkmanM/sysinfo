use clap::Parser;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Summarise Linux memory, swap, uptime, load averages and process counts"
)]
struct Args {
    #[arg(long, help = "Display exact memory byte counts")]
    bytes: bool,
    #[arg(long, help = "Display JSON with numeric memory byte counts")]
    json: bool,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Snapshot, memory_bytes};
    use std::ffi::c_int;
    #[cfg(not(all(target_arch = "x86_64", target_pointer_width = "32")))]
    use std::ffi::{c_long, c_ulong};
    use std::mem::MaybeUninit;

    #[cfg(not(all(target_arch = "x86_64", target_pointer_width = "32")))]
    type KernelLong = c_long;
    #[cfg(not(all(target_arch = "x86_64", target_pointer_width = "32")))]
    type KernelUlong = c_ulong;
    #[cfg(all(target_arch = "x86_64", target_pointer_width = "32"))]
    type KernelLong = i64;
    #[cfg(all(target_arch = "x86_64", target_pointer_width = "32"))]
    type KernelUlong = u64;

    #[repr(C)]
    struct Sysinfo {
        uptime: KernelLong,
        loads: [KernelUlong; 3],
        totalram: KernelUlong,
        freeram: KernelUlong,
        sharedram: KernelUlong,
        bufferram: KernelUlong,
        totalswap: KernelUlong,
        freeswap: KernelUlong,
        procs: u16,
        pad: u16,
        totalhigh: KernelUlong,
        freehigh: KernelUlong,
        mem_unit: u32,
        reserved: [u8; 20 - 2 * size_of::<KernelUlong>() - size_of::<u32>()],
    }

    unsafe extern "C" {
        fn sysinfo(info: *mut Sysinfo) -> c_int;
    }

    pub(super) fn query() -> Result<Snapshot, String> {
        let mut raw = MaybeUninit::<Sysinfo>::zeroed();
        if unsafe { sysinfo(raw.as_mut_ptr()) } != 0 {
            return Err(format!(
                "sysinfo query failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let raw = unsafe { raw.assume_init() };
        let uptime_seconds = u64::try_from(raw.uptime)
            .map_err(|_| "kernel returned a negative uptime".to_owned())?;
        Ok(Snapshot {
            uptime_seconds,
            loads: raw.loads.map(super::decode_load),
            processes: raw.procs,
            total_ram_bytes: memory_bytes(raw.totalram, raw.mem_unit, "total RAM")?,
            free_ram_bytes: memory_bytes(raw.freeram, raw.mem_unit, "free RAM")?,
            total_swap_bytes: memory_bytes(raw.totalswap, raw.mem_unit, "total swap")?,
            free_swap_bytes: memory_bytes(raw.freeswap, raw.mem_unit, "free swap")?,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn matches_linux_abi_layout() {
            let wide = size_of::<KernelUlong>() == 8;
            assert_eq!(size_of::<Sysinfo>(), if wide { 112 } else { 64 });
            assert_eq!(
                std::mem::offset_of!(Sysinfo, procs),
                if wide { 80 } else { 40 }
            );
            assert_eq!(
                std::mem::offset_of!(Sysinfo, mem_unit),
                if wide { 104 } else { 52 }
            );
        }
    }
}

#[derive(Debug)]
struct Snapshot {
    uptime_seconds: u64,
    loads: [f64; 3],
    processes: u16,
    total_ram_bytes: u64,
    free_ram_bytes: u64,
    total_swap_bytes: u64,
    free_swap_bytes: u64,
}

#[cfg(any(target_os = "linux", test))]
fn memory_bytes(value: impl Into<u64>, unit: u32, label: &str) -> Result<u64, String> {
    if unit == 0 {
        return Err("kernel returned a zero memory unit".to_owned());
    }
    value
        .into()
        .checked_mul(u64::from(unit))
        .ok_or_else(|| format!("{label} byte count exceeds the supported range"))
}

#[cfg(any(target_os = "linux", test))]
fn decode_load(value: impl Into<u64>) -> f64 {
    value.into() as f64 / 65_536.0
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\u{0000}'..='\u{001f}' => {
                write!(result, "\\u{:04x}", character as u32).unwrap();
            }
            _ => result.push(character),
        }
    }
    result.push('"');
    result
}

fn format_memory(bytes: u64, exact: bool) -> String {
    if exact || bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", units[unit])
}

impl Snapshot {
    fn render(&self, args: &Args) -> String {
        if args.json {
            return format!(
                "{{\"platform\":{},\"uptime_seconds\":{},\"load_1\":{},\"load_5\":{},\"load_15\":{},\"processes\":{},\"total_ram_bytes\":{},\"free_ram_bytes\":{},\"total_swap_bytes\":{},\"free_swap_bytes\":{}}}",
                json_string("linux"),
                self.uptime_seconds,
                self.loads[0],
                self.loads[1],
                self.loads[2],
                self.processes,
                self.total_ram_bytes,
                self.free_ram_bytes,
                self.total_swap_bytes,
                self.free_swap_bytes,
            );
        }
        let seconds = self.uptime_seconds;
        format!(
            "Uptime: {}d {:02}h {:02}m {:02}s\nLoad averages (1, 5, 15 min): {:.2}, {:.2}, {:.2}\nProcesses: {}\nTotal RAM: {}\nFree RAM: {}\nTotal swap: {}\nFree swap: {}",
            seconds / 86_400,
            seconds / 3_600 % 24,
            seconds / 60 % 60,
            seconds % 60,
            self.loads[0],
            self.loads[1],
            self.loads[2],
            self.processes,
            format_memory(self.total_ram_bytes, args.bytes),
            format_memory(self.free_ram_bytes, args.bytes),
            format_memory(self.total_swap_bytes, args.bytes),
            format_memory(self.free_swap_bytes, args.bytes),
        )
    }
}

fn query() -> Result<Snapshot, String> {
    #[cfg(target_os = "linux")]
    {
        linux::query()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("unsupported platform: sysinfo requires Linux".to_owned())
    }
}

fn run(args: &Args) -> Result<(), String> {
    let snapshot = query()?;
    writeln!(io::stdout().lock(), "{}", snapshot.render(args))
        .map_err(|error| format!("could not write output: {error}"))
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sysinfo: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            uptime_seconds: 93_784,
            loads: [0.5, 1.25, 2.0],
            processes: 42,
            total_ram_bytes: 8_192,
            free_ram_bytes: 4_096,
            total_swap_bytes: 0,
            free_swap_bytes: 0,
        }
    }

    #[test]
    fn memory_conversion_checks_units_and_overflow() {
        assert_eq!(memory_bytes(3_u64, 4096, "RAM").unwrap(), 12_288);
        assert_eq!(memory_bytes(0_u64, 4096, "swap").unwrap(), 0);
        assert_eq!(memory_bytes(u64::MAX, 1, "RAM").unwrap(), u64::MAX);
        assert!(memory_bytes(u64::MAX, 2, "RAM").is_err());
        assert!(memory_bytes(0_u64, 0, "RAM").is_err());
    }

    #[test]
    fn loads_decode_sixteen_fractional_bits() {
        assert_eq!(decode_load(0_u64), 0.0);
        assert_eq!(decode_load(81_920_u64), 1.25);
        assert_eq!(decode_load(1_u64), 1.0 / 65_536.0);
    }

    #[test]
    fn json_escapes_strings_and_control_characters() {
        assert_eq!(
            json_string("\"\\\n\r\t\u{0008}\0£"),
            "\"\\\"\\\\\\n\\r\\t\\u0008\\u0000£\""
        );
    }

    #[test]
    fn json_has_stable_numeric_fields() {
        let args = Args::try_parse_from(["sysinfo", "--json"]).unwrap();
        let expected = "{\"platform\":\"linux\",\"uptime_seconds\":93784,\"load_1\":0.5,\"load_5\":1.25,\"load_15\":2,\"processes\":42,\"total_ram_bytes\":8192,\"free_ram_bytes\":4096,\"total_swap_bytes\":0,\"free_swap_bytes\":0}";
        assert_eq!(sample().render(&args), expected);
        let combined = Args::try_parse_from(["sysinfo", "--json", "--bytes"]).unwrap();
        assert_eq!(sample().render(&combined), expected);
    }

    #[test]
    fn human_output_formats_uptime_and_exact_bytes() {
        let args = Args::try_parse_from(["sysinfo", "--bytes"]).unwrap();
        let output = sample().render(&args);
        assert!(output.starts_with("Uptime: 1d 02h 03m 04s\n"));
        assert!(output.contains("Total RAM: 8192 B\n"));
        assert!(output.ends_with("Free swap: 0 B"));
        assert_eq!(format_memory(1536, false), "1.50 KiB");
        assert_eq!(format_memory(0, false), "0 B");
        assert_eq!(format_memory(u64::MAX, true), "18446744073709551615 B");
    }

    #[test]
    fn parser_rejects_unknown_arguments_and_values() {
        for arguments in [
            vec!["sysinfo", "--unknown"],
            vec!["sysinfo", "file"],
            vec!["sysinfo", "--bytes=invalid"],
        ] {
            assert!(Args::try_parse_from(arguments).is_err());
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_platform_is_explicit() {
        assert_eq!(
            query().unwrap_err(),
            "unsupported platform: sysinfo requires Linux"
        );
    }
}
