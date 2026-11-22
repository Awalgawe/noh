//! What the machine has free when the agent opens a NOH window, and the cap
//! that follows (agent-follow note §5, formula granted by the maintainer).
//!
//! A window that just opened weighs little; once its project is loaded it
//! weighs about 450–505 MiB and 202 MiB of graphics memory. So each window
//! already open keeps a margin: what it may still take up to its budget.
//! The processor does not count: one heavy job at a time and the preparation
//! turn keep its load independent of the number of windows.
use rmcp::schemars::{self, JsonSchema};
use std::collections::HashMap;

pub const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
/// One window's memory budget after an hour (note §5, budgets).
pub const WINDOW_MEMORY: u64 = 550 * MIB;
/// One window's graphics memory budget.
pub const WINDOW_VRAM: u64 = 250 * MIB;
/// Graphics memory left to the other programs (ComfyUI, LM Studio, the desktop).
pub const VRAM_RESERVE: u64 = GIB;
/// At most this many windows per server, whatever is left: beyond, nobody
/// follows them.
pub const CEILING: usize = 8;
/// The ceiling when graphics memory cannot be read.
pub const CEILING_WITHOUT_VRAM: usize = 4;
/// The ceiling when memory cannot be read either (decided by the pair).
pub const CEILING_UNMEASURED: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub available: u64,
    pub total: u64,
}

/// The machine at the moment of an open; `None` where it cannot be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    pub memory: Option<Memory>,
    /// Free dedicated graphics memory of the adapter windows draw on.
    pub vram_free: Option<u64>,
    /// Dedicated graphics memory used by each process, by pid.
    pub vram_by_pid: HashMap<u32, u64>,
}

/// What an open window uses now; `None` where it cannot be read (then its
/// whole budget is kept as its margin).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub private: Option<u64>,
    pub vram: Option<u64>,
}

/// Why one more window was refused, with the figures that say so.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, JsonSchema)]
pub struct CapRefusal {
    /// `memory`, `graphics_memory` or `ceiling`.
    pub resource: String,
    /// Bytes free now (windows open, for the ceiling).
    pub free: u64,
    /// Bytes one more window needs, margins and reserve included (the
    /// ceiling, for the ceiling).
    pub needed: u64,
    pub open: usize,
}

impl CapRefusal {
    pub fn message(&self) -> String {
        let gib = |bytes: u64| bytes as f64 / GIB as f64;
        match self.resource.as_str() {
            "ceiling" => format!(
                "{} NOH windows are open, the most one session opens on this machine.",
                self.open
            ),
            "graphics_memory" => format!(
                "{:.1} GiB of graphics memory free, {:.1} GiB needed for one more NOH window.",
                gib(self.free),
                gib(self.needed)
            ),
            _ => format!(
                "{:.1} GiB free, {:.1} GiB needed for one more NOH window.",
                gib(self.free),
                gib(self.needed)
            ),
        }
    }
}

/// The memory kept for the system and the other programs: 2 GiB or 15 % of
/// the total, whichever is larger.
pub fn memory_reserve(total: u64) -> u64 {
    (2 * GIB).max(total / 100 * 15)
}

/// One more window, given the machine and the windows already open.
pub fn admit(machine: &Machine, open: &[Usage]) -> Result<(), CapRefusal> {
    let ceiling = match (machine.memory, machine.vram_free) {
        (None, _) => CEILING_UNMEASURED,
        (Some(_), None) => CEILING_WITHOUT_VRAM,
        (Some(_), Some(_)) => CEILING,
    };
    if open.len() >= ceiling {
        return Err(CapRefusal {
            resource: "ceiling".into(),
            free: open.len() as u64,
            needed: ceiling as u64,
            open: open.len(),
        });
    }
    if let Some(memory) = machine.memory {
        let margins: u64 = open
            .iter()
            .map(|u| WINDOW_MEMORY.saturating_sub(u.private.unwrap_or(0)))
            .sum();
        let needed = margins + WINDOW_MEMORY + memory_reserve(memory.total);
        if memory.available < needed {
            return Err(CapRefusal {
                resource: "memory".into(),
                free: memory.available,
                needed,
                open: open.len(),
            });
        }
    }
    if let Some(free) = machine.vram_free {
        let margins: u64 = open
            .iter()
            .map(|u| WINDOW_VRAM.saturating_sub(u.vram.unwrap_or(0)))
            .sum();
        let needed = margins + WINDOW_VRAM + VRAM_RESERVE;
        if free < needed {
            return Err(CapRefusal {
                resource: "graphics_memory".into(),
                free,
                needed,
                open: open.len(),
            });
        }
    }
    Ok(())
}

/// Read the machine now. Blocking (tens of milliseconds on Windows).
pub fn read() -> Machine {
    platform::read()
}

/// The private memory of the process behind `child`, if it can be read.
pub fn private_memory(child: &std::process::Child) -> Option<u64> {
    platform::private_memory(child)
}

#[cfg(windows)]
mod platform {
    //! Memory: GlobalMemoryStatusEx; a process: its PrivateUsage. Graphics
    //! memory: DXGI gives the adapters' dedicated totals but no free figure
    //! (`QueryVideoMemoryInfo` reports the calling process only), so the free
    //! memory is the total of the adapter with the most dedicated memory minus
    //! every process's dedicated usage on it, from the « GPU Adapter Memory »
    //! counter (English names: the counters are localized). DXGI and PDH are
    //! loaded when first read, so a server that never opens a window never
    //! loads them; no crate binding is added for them.
    use super::{Machine, Memory};
    use std::{collections::HashMap, ffi::c_void, os::windows::io::AsRawHandle};

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn K32GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    pub fn read() -> Machine {
        let mut machine = Machine {
            memory: memory(),
            ..Default::default()
        };
        if let Some((luid, total)) = adapter() {
            let (used, by_pid) = counters(luid).unwrap_or_default();
            if let Some(used) = used {
                machine.vram_free = Some(total.saturating_sub(used));
            }
            machine.vram_by_pid = by_pid;
        }
        machine
    }

    fn memory() -> Option<Memory> {
        let mut status = MemoryStatusEx {
            length: size_of::<MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(Memory {
            available: status.avail_phys,
            total: status.total_phys,
        })
    }

    pub fn private_memory(child: &std::process::Child) -> Option<u64> {
        let mut counters = ProcessMemoryCountersEx {
            cb: size_of::<ProcessMemoryCountersEx>() as u32,
            ..Default::default()
        };
        (unsafe {
            K32GetProcessMemoryInfo(child.as_raw_handle().cast(), &mut counters, counters.cb)
        } != 0)
            .then_some(counters.private_usage as u64)
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn function(library: &str, name: &str) -> Option<*mut c_void> {
        let module = unsafe { LoadLibraryW(wide(library).as_ptr()) };
        if module.is_null() {
            return None;
        }
        let mut name = name.as_bytes().to_vec();
        name.push(0);
        let address = unsafe { GetProcAddress(module, name.as_ptr()) };
        (!address.is_null()).then_some(address)
    }

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const IID_IDXGI_FACTORY1: Guid = Guid(
        0x770a_ae78,
        0xf26f,
        0x4dba,
        [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    );
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Luid {
        low: u32,
        high: i32,
    }
    #[repr(C)]
    struct AdapterDesc1 {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        adapter_luid: Luid,
        flags: u32,
    }
    const ADAPTER_FLAG_SOFTWARE: u32 = 2;
    type CreateFactory = unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    type EnumAdapters1 = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
    type GetDesc1 = unsafe extern "system" fn(*mut c_void, *mut AdapterDesc1) -> i32;

    /// The vtable entry `index` of the COM object `object`.
    unsafe fn method<T: Copy>(object: *mut c_void, index: usize) -> T {
        unsafe {
            let table = *(object as *const *const *const c_void);
            std::mem::transmute_copy(&*table.add(index))
        }
    }

    /// The hardware adapter with the most dedicated memory: its LUID and total.
    fn adapter() -> Option<(Luid, u64)> {
        let create: CreateFactory =
            unsafe { std::mem::transmute(function("dxgi.dll", "CreateDXGIFactory1")?) };
        let mut factory = std::ptr::null_mut();
        if unsafe { create(&IID_IDXGI_FACTORY1, &mut factory) } < 0 || factory.is_null() {
            return None;
        }
        let mut best: Option<(Luid, u64)> = None;
        unsafe {
            // IDXGIFactory1: IUnknown (3), IDXGIObject (4), IDXGIFactory (5), then EnumAdapters1.
            let enumerate: EnumAdapters1 = method(factory, 12);
            for index in 0..16 {
                let mut adapter = std::ptr::null_mut();
                if enumerate(factory, index, &mut adapter) < 0 || adapter.is_null() {
                    break;
                }
                // IDXGIAdapter1: IUnknown (3), IDXGIObject (4), IDXGIAdapter (3), then GetDesc1.
                let describe: GetDesc1 = method(adapter, 10);
                let mut desc: AdapterDesc1 = std::mem::zeroed();
                if describe(adapter, &mut desc) >= 0
                    && desc.flags & ADAPTER_FLAG_SOFTWARE == 0
                    && best.is_none_or(|(_, total)| desc.dedicated_video_memory as u64 > total)
                {
                    best = Some((desc.adapter_luid, desc.dedicated_video_memory as u64));
                }
                let release: Release = method(adapter, 2);
                release(adapter);
            }
            let release: Release = method(factory, 2);
            release(factory);
        }
        best.filter(|(_, total)| *total > 0)
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CounterValue {
        status: u32,
        value: i64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CounterItem {
        name: *const u16,
        value: CounterValue,
    }
    const FMT_LARGE: u32 = 0x0000_0400;
    const MORE_DATA: u32 = 0x8000_07D2;
    type OpenQuery = unsafe extern "system" fn(*const u16, usize, *mut isize) -> u32;
    type AddCounter = unsafe extern "system" fn(isize, *const u16, usize, *mut isize) -> u32;
    type Collect = unsafe extern "system" fn(isize) -> u32;
    type CounterArray =
        unsafe extern "system" fn(isize, u32, *mut u32, *mut u32, *mut CounterItem) -> u32;
    type CloseQuery = unsafe extern "system" fn(isize) -> u32;

    /// The adapter's dedicated usage by every process, and each process's.
    #[allow(clippy::type_complexity)]
    fn counters(luid: Luid) -> Option<(Option<u64>, HashMap<u32, u64>)> {
        unsafe {
            let open: OpenQuery = std::mem::transmute(function("pdh.dll", "PdhOpenQueryW")?);
            let add: AddCounter =
                std::mem::transmute(function("pdh.dll", "PdhAddEnglishCounterW")?);
            let collect: Collect = std::mem::transmute(function("pdh.dll", "PdhCollectQueryData")?);
            let array: CounterArray =
                std::mem::transmute(function("pdh.dll", "PdhGetFormattedCounterArrayW")?);
            let close: CloseQuery = std::mem::transmute(function("pdh.dll", "PdhCloseQuery")?);
            let mut query = 0isize;
            if open(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut adapter_counter = 0isize;
            let mut process_counter = 0isize;
            let adapters = add(
                query,
                wide(r"\GPU Adapter Memory(*)\Dedicated Usage").as_ptr(),
                0,
                &mut adapter_counter,
            ) == 0;
            let processes = add(
                query,
                wide(r"\GPU Process Memory(*)\Dedicated Usage").as_ptr(),
                0,
                &mut process_counter,
            ) == 0;
            let mut result = (None, HashMap::new());
            if collect(query) == 0 {
                let tag = format!("luid_0x{:08x}_0x{:08x}", luid.high as u32, luid.low);
                if adapters {
                    result.0 = items(array, adapter_counter).map(|items| {
                        items
                            .iter()
                            .filter(|(name, _)| name.to_ascii_lowercase().starts_with(&tag))
                            .map(|(_, value)| *value)
                            .sum()
                    });
                }
                if processes && let Some(items) = items(array, process_counter) {
                    for (name, value) in items {
                        let name = name.to_ascii_lowercase();
                        // pid_<pid>_luid_0x…_0x…_phys_0
                        if let Some(rest) = name.strip_prefix("pid_")
                            && let Some((pid, adapter)) = rest.split_once('_')
                            && adapter.starts_with(&tag)
                            && let Ok(pid) = pid.parse::<u32>()
                        {
                            *result.1.entry(pid).or_insert(0) += value;
                        }
                    }
                }
            }
            close(query);
            Some(result)
        }
    }

    /// Every instance of a wildcard counter, with its value.
    unsafe fn items(array: CounterArray, counter: isize) -> Option<Vec<(String, u64)>> {
        unsafe {
            let mut size = 0u32;
            let mut count = 0u32;
            let status = array(
                counter,
                FMT_LARGE,
                &mut size,
                &mut count,
                std::ptr::null_mut(),
            );
            if status != MORE_DATA || size == 0 {
                return (status == 0).then(Vec::new);
            }
            // The buffer holds the items, then their names: aligned for the items.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<CounterItem>();
            if array(counter, FMT_LARGE, &mut size, &mut count, items) != 0 {
                return None;
            }
            let mut out = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                let item = *items.add(index);
                if item.name.is_null() || item.value.status > 1 {
                    continue;
                }
                let mut length = 0;
                while *item.name.add(length) != 0 {
                    length += 1;
                }
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(item.name, length));
                out.push((name, item.value.value.max(0) as u64));
            }
            Some(out)
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    //! Memory from /proc; graphics memory depends on the driver and is not read.
    use super::{Machine, Memory};

    fn kib(text: &str, key: &str) -> Option<u64> {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|rest| {
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map(|k| k * 1024)
    }

    pub fn read() -> Machine {
        let memory = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                Some(Memory {
                    available: kib(&text, "MemAvailable:")?,
                    total: kib(&text, "MemTotal:")?,
                })
            });
        Machine {
            memory,
            ..Default::default()
        }
    }

    pub fn private_memory(child: &std::process::Child) -> Option<u64> {
        let status = std::fs::read_to_string(format!("/proc/{}/status", child.id())).ok()?;
        kib(&status, "RssAnon:")
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    //! Not measured here yet: the unmeasured ceiling applies.
    use super::Machine;

    pub fn read() -> Machine {
        Machine::default()
    }

    pub fn private_memory(_: &std::process::Child) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Figures in decimal gigabytes, as the note's examples give them.
    fn machine(available_gb: f64, total_gb: f64, vram_free_gb: Option<f64>) -> Machine {
        let bytes = |gb: f64| (gb * 1e9) as u64;
        Machine {
            memory: Some(Memory {
                available: bytes(available_gb),
                total: bytes(total_gb),
            }),
            vram_free: vram_free_gb.map(bytes),
            vram_by_pid: HashMap::new(),
        }
    }

    /// Windows that just opened: about 127 MiB and 20 MiB of graphics memory.
    fn fresh() -> Usage {
        Usage {
            private: Some(127 * MIB),
            vram: Some(20 * MIB),
        }
    }

    fn count(machine: &Machine, each: Usage) -> usize {
        let mut open = Vec::new();
        while admit(machine, &open).is_ok() {
            open.push(each);
            assert!(open.len() <= CEILING);
        }
        open.len()
    }

    #[test]
    fn this_machine_meets_the_readability_ceiling() {
        // 19.3 GB free of 32, 6.9 GB of graphics memory free (note §5).
        let m = machine(19.3, 32.0, Some(6.9));
        assert_eq!(count(&m, fresh()), CEILING);
        let refusal = admit(&m, &[fresh(); CEILING]).unwrap_err();
        assert_eq!(refusal.resource, "ceiling");
        assert!(refusal.message().starts_with("8 NOH windows are open"));
    }

    #[test]
    fn a_small_laptop_opens_three_and_refuses_the_fourth_at_once() {
        // 8 GB, 4 GB free, graphics memory unknown: (4 − 2) ÷ 0.55, so 3,
        // the fourth refused right after because the margins count.
        let m = machine(4.0, 8.0, None);
        assert_eq!(count(&m, fresh()), 3);
        let refusal = admit(&m, &[fresh(); 3]).unwrap_err();
        assert_eq!(refusal.resource, "memory");
        assert_eq!(refusal.needed, 3 * (550 - 127) * MIB + 550 * MIB + 2 * GIB);
        assert!(
            refusal.message().contains("3.7 GiB free") && refusal.message().contains("needed"),
            "{}",
            refusal.message()
        );
        // Loaded windows past their budget keep no margin.
        let loaded = Usage {
            private: Some(600 * MIB),
            vram: None,
        };
        assert!(admit(&m, &[loaded; 3]).is_ok());
    }

    #[test]
    fn the_reserve_is_two_gib_or_fifteen_percent() {
        assert_eq!(memory_reserve(8 * GIB), 2 * GIB);
        assert_eq!(memory_reserve(32 * GIB), 32 * GIB / 100 * 15);
    }

    #[test]
    fn graphics_memory_refuses_with_its_own_reason() {
        // Plenty of memory, 1.5 GiB of graphics memory free: 1 GiB reserve,
        // 250 MiB per window, the margins of the open ones.
        let m = machine(20.0, 32.0, Some(1.5));
        assert_eq!(count(&m, fresh()), 1);
        let refusal = admit(&m, &[fresh()]).unwrap_err();
        assert_eq!(refusal.resource, "graphics_memory");
        assert!(refusal.message().contains("graphics memory"));
    }

    #[test]
    fn unknown_figures_lower_the_ceiling_and_unknown_usage_keeps_the_whole_margin() {
        assert_eq!(
            count(&machine(30.0, 32.0, None), fresh()),
            CEILING_WITHOUT_VRAM
        );
        assert_eq!(
            count(&Machine::default(), Usage::default()),
            CEILING_UNMEASURED
        );
        // An open window whose memory cannot be read keeps its whole budget.
        let m = machine(2.0 + 1.1 + 0.05, 8.0, None);
        assert!(admit(&m, &[]).is_ok());
        assert!(admit(&m, &[Usage::default()]).is_err());
    }

    #[test]
    fn reading_this_machine_gives_memory() {
        let m = read();
        // Shown with --nocapture: compared by hand with nvidia-smi.
        eprintln!(
            "memory {:?} MiB available of {:?}; graphics memory free {:?} MiB",
            m.memory.map(|m| m.available / MIB),
            m.memory.map(|m| m.total / MIB),
            m.vram_free.map(|v| v / MIB)
        );
        if cfg!(any(windows, target_os = "linux")) {
            let memory = m.memory.expect("memory is read on this system");
            assert!(memory.available > 0 && memory.available <= memory.total);
        }
        if let Some(free) = m.vram_free {
            assert!(free < 1024 * GIB);
        }
    }
}
