//! GPU info and per-process memory reading from sysfs and /proc/*/fdinfo.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Static GPU information from sysfs.
#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub driver_version: String,
    pub vram_total: u64,
    pub gtt_total: u64,
    pub card_path: PathBuf,
}

/// Dynamic GPU stats read each tick.
#[derive(Debug, Clone, Default)]
pub struct GpuStats {
    pub vram_used: u64,
    pub gtt_used: u64,
    pub gpu_busy_pct: u8,
    pub temp_millicelsius: u32,
    pub sclk_mhz: u32,
    pub mclk_mhz: u32,
}

/// Per-process GPU memory usage.
#[derive(Debug, Clone)]
pub struct ProcessGpuMem {
    pub pid: u32,
    pub name: String,
    pub vram_kib: u64,
    pub gtt_kib: u64,
}

impl ProcessGpuMem {
    pub fn total_kib(&self) -> u64 {
        self.vram_kib + self.gtt_kib
    }
}

/// Find the amdgpu card sysfs path (picks the first with mem_info_vram_total).
pub fn find_gpu() -> Result<GpuInfo> {
    for entry in fs::read_dir("/sys/class/drm")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let dev = entry.path().join("device");
        let vram_path = dev.join("mem_info_vram_total");
        if vram_path.exists() {
            let vram_total = read_sysfs_u64(&vram_path)?;
            let gtt_total = read_sysfs_u64(&dev.join("mem_info_gtt_total"))?;
            let device_id = read_sysfs_str(&dev.join("device")).unwrap_or_default();
            let gpu_name = pci_id_to_name(&device_id);
            let driver_version = detect_driver_version(&dev);
            return Ok(GpuInfo {
                name: gpu_name,
                driver_version,
                vram_total,
                gtt_total,
                card_path: dev,
            });
        }
    }
    anyhow::bail!("No AMD GPU with VRAM sysfs found in /sys/class/drm")
}

fn detect_driver_version(dev_path: &Path) -> String {
    // Try module version first
    if let Ok(v) = read_sysfs_str(&dev_path.join("driver/module/version")) {
        if !v.is_empty() {
            return v;
        }
    }
    // Try reading from modinfo via fw_version sysfs
    if let Ok(v) = read_sysfs_str(&dev_path.join("fw_version")) {
        if !v.is_empty() {
            return format!("fw:{}", v);
        }
    }
    // Fall back to kernel version
    if let Ok(v) = read_sysfs_str(Path::new("/proc/sys/kernel/osrelease")) {
        return format!("kernel:{}", v);
    }
    "unknown".to_string()
}

fn pci_id_to_name(device_id: &str) -> String {
    let id = device_id.trim().trim_start_matches("0x");
    match id {
        "150e" => "AMD Radeon 890M (gfx1150)".to_string(),
        "1900" => "AMD Radeon RX 6900 XT".to_string(),
        "73bf" => "AMD Radeon RX 6900 XT".to_string(),
        "164e" => "AMD Radeon 780M".to_string(),
        _ => format!("AMD GPU [{}]", id),
    }
}

/// Read dynamic GPU stats.
pub fn read_stats(gpu: &GpuInfo) -> GpuStats {
    let p = &gpu.card_path;
    GpuStats {
        vram_used: read_sysfs_u64(&p.join("mem_info_vram_used")).unwrap_or(0),
        gtt_used: read_sysfs_u64(&p.join("mem_info_gtt_used")).unwrap_or(0),
        gpu_busy_pct: read_sysfs_u64(&p.join("gpu_busy_percent")).unwrap_or(0) as u8,
        temp_millicelsius: find_hwmon_temp(p).unwrap_or(0),
        sclk_mhz: parse_active_dpm(&p.join("pp_dpm_sclk")).unwrap_or(0),
        mclk_mhz: parse_active_dpm(&p.join("pp_dpm_mclk")).unwrap_or(0),
    }
}

fn find_hwmon_temp(dev_path: &Path) -> Option<u32> {
    let hwmon = dev_path.join("hwmon");
    for entry in fs::read_dir(hwmon).ok()? {
        let entry = entry.ok()?;
        let temp_path = entry.path().join("temp1_input");
        if temp_path.exists() {
            return read_sysfs_u64(&temp_path).ok().map(|v| v as u32);
        }
    }
    None
}

fn parse_active_dpm(path: &Path) -> Option<u32> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if line.contains('*') {
            // Format: "1: 742Mhz *"
            let mhz_part = line.split_whitespace().nth(1)?;
            let num_str = mhz_part.trim_end_matches("Mhz").trim_end_matches("MHz");
            return num_str.parse().ok();
        }
    }
    None
}

/// Scan /proc/*/fdinfo/* for drm-memory-vram and drm-memory-gtt.
/// Deduplicates per PID (takes max values across fds since multiple fds
/// in the same process share the same DRM context and report identical values).
pub fn read_process_gpu_mem() -> Vec<ProcessGpuMem> {
    let mut map: HashMap<u32, ProcessGpuMem> = HashMap::new();

    let proc_dir = match fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let fdinfo_path = entry.path().join("fdinfo");
        let fdinfo_dir = match fs::read_dir(&fdinfo_path) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let mut best_vram: u64 = 0;
        let mut best_gtt: u64 = 0;
        let mut found = false;

        for fd_entry in fdinfo_dir.flatten() {
            let content = match fs::read_to_string(fd_entry.path()) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let mut vram: u64 = 0;
            let mut gtt: u64 = 0;
            let mut has_drm = false;

            for line in content.lines() {
                if let Some(rest) = line.strip_prefix("drm-memory-vram:") {
                    if let Some(val) = parse_kib_value(rest) {
                        vram = val;
                        has_drm = true;
                    }
                } else if let Some(rest) = line.strip_prefix("drm-memory-gtt:") {
                    if let Some(val) = parse_kib_value(rest) {
                        gtt = val;
                    }
                }
            }

            if has_drm {
                found = true;
                // Take the max — all fds in the same DRM context report the same values,
                // but there might be multiple contexts (rare).
                best_vram = best_vram.max(vram);
                best_gtt = best_gtt.max(gtt);
            }
        }

        if found {
            let comm = fs::read_to_string(format!("/proc/{}/comm", pid))
                .unwrap_or_else(|_| "?".to_string())
                .trim()
                .to_string();

            let entry = map.entry(pid).or_insert_with(|| ProcessGpuMem {
                pid,
                name: comm,
                vram_kib: 0,
                gtt_kib: 0,
            });
            entry.vram_kib = entry.vram_kib.max(best_vram);
            entry.gtt_kib = entry.gtt_kib.max(best_gtt);
        }
    }

    let mut procs: Vec<ProcessGpuMem> = map.into_values().collect();
    procs.sort_by(|a, b| b.total_kib().cmp(&a.total_kib()));
    procs
}

fn parse_kib_value(s: &str) -> Option<u64> {
    // Format: "\t12345 KiB" or " 12345 KiB"
    let trimmed = s.trim();
    let num_str = trimmed.split_whitespace().next()?;
    num_str.parse().ok()
}

fn read_sysfs_u64(path: &Path) -> Result<u64> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    content
        .trim()
        .parse()
        .with_context(|| format!("parsing {} from {}", content.trim(), path.display()))
}

fn read_sysfs_str(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?
        .trim()
        .to_string())
}
