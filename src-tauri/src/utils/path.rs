use std::path::{Path, PathBuf};
use super::constants::EXE_VERSION_READ_LIMIT;

/// 获取应用数据目录
pub fn get_app_data_dir() -> PathBuf {
    let mut path = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("GameVault");
    path
}

/// 获取应用配置目录
pub fn get_app_config_dir() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("GameVault");
    path
}

/// 获取数据库路径
pub fn get_database_path() -> PathBuf {
    get_app_data_dir().join("gamevault.db")
}

/// 获取封面缓存目录
pub fn get_covers_dir() -> PathBuf {
    get_app_data_dir().join("covers")
}

/// 确保目录存在
pub fn ensure_dir_exists(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// 展开路径中的 Windows 环境变量（如 %APPDATA%、%USERPROFILE% 等）
pub fn expand_env_vars(path: &str) -> String {
    let mut result = path.to_string();

    // 匹配 %VARNAME% 格式的环境变量
    let mut start = 0;
    while let Some(prefix_pos) = result[start..].find('%') {
        let abs_prefix = start + prefix_pos;
        if let Some(suffix_pos) = result[abs_prefix + 1..].find('%') {
            let abs_suffix = abs_prefix + 1 + suffix_pos;
            let var_name = &result[abs_prefix + 1..abs_suffix];
            if let Ok(var_value) = std::env::var(var_name) {
                result = format!("{}{}{}", &result[..abs_prefix], var_value, &result[abs_suffix + 1..]);
                start = abs_prefix + var_value.len();
            } else {
                start = abs_suffix + 1;
            }
        } else {
            break;
        }
    }

    // 处理 ~ 路径（Unix 风格，部分 Windows 工具也支持）
    if result.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            if result.len() == 1 || result.starts_with("~/") || result.starts_with("~\\") {
                result = format!("{}{}", home.display(), &result[1..]);
            }
        }
    }

    result
}

/// 从 Windows PE 文件中读取 FileVersion 版本号
/// 返回值如 "1.2.3.4" 或 None（非 PE 文件或无版本信息）
/// 只读取前 1MB，避免将整个 EXE（可能数百 MB）加载到内存
pub fn read_exe_version(path: &str) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut data = Vec::with_capacity(EXE_VERSION_READ_LIMIT as usize);
    let mut limited = file.take(EXE_VERSION_READ_LIMIT);
    limited.read_to_end(&mut data).ok()?;

    if data.len() < 64 || &data[0..2] != b"MZ" {
        return None;
    }

    let pe_off = read_u32(&data, 60)? as usize;
    if pe_off + 24 > data.len() || &data[pe_off..pe_off + 4] != b"PE\0\0" {
        return None;
    }

    let coff = pe_off + 4;
    let num_sections = read_u16(&data, coff + 2)? as usize;
    let opt_size = read_u16(&data, coff + 16)? as usize;
    let opt = coff + 20;

    if opt + opt_size > data.len() {
        return None;
    }

    let magic = read_u16(&data, opt)?;
    let (dd_off, dd_cnt) = match magic {
        0x10b => (opt + 96, read_u32(&data, opt + 92)? as usize),
        0x20b => (opt + 112, read_u32(&data, opt + 108)? as usize),
        _ => return None,
    };

    // 资源目录是第 3 个数据目录（索引 2），每个条目 8 字节
    if dd_cnt < 3 || dd_off + 24 > data.len() {
        return None;
    }
    let res_rva = read_u32(&data, dd_off + 16)? as usize;
    if res_rva == 0 {
        return None;
    }

    let sections = opt + opt_size;
    let res_file = rva_to_offset(&data, sections, num_sections, res_rva)?;

    // 遍历资源目录树：在 depth=0 按 ID=16（RT_VERSION）查找
    let data_entry_file = find_version_resource(&data, res_file, res_file, 0)?;

    // data_entry_file 指向资源数据条目，偏移 0 是 DataRVA
    let data_rva = read_u32(&data, data_entry_file)? as usize;
    let ver_file = rva_to_offset(&data, sections, num_sections, data_rva)?;

    let size = read_u32(&data, ver_file + 4)? as usize;
    if size < 52 {
        return None;
    }

    // VS_FIXEDFILEINFO 位于偏移 40（VS_VERSIONINFO 头 + Key("VS_VERSION_INFO") + 对齐）
    let ffi = ver_file + 40;
    if ffi + 52 > data.len() || read_u32(&data, ffi)? != 0xFEEF04BD {
        return None;
    }

    // dwFileVersionMS = (Major << 16) | Minor → 低 16 位在前（小端序）
    let minor = read_u16(&data, ffi + 8)?;
    let major = read_u16(&data, ffi + 10)?;
    // dwFileVersionLS = (Build << 16) | Patch
    let patch = read_u16(&data, ffi + 12)?;
    let build = read_u16(&data, ffi + 14)?;

    Some(format!("{}.{}.{}.{}", major, minor, build, patch))
}

/// 递归遍历资源目录树，返回 RT_VERSION 数据条目的文件偏移
/// `base` 是资源节在文件中的起始偏移，所有资源树内的偏移都相对于此基址
fn find_version_resource(data: &[u8], base: usize, dir_file: usize, depth: u32) -> Option<usize> {
    if depth > 3 || dir_file + 16 > data.len() {
        return None;
    }

    let named = read_u16(&data, dir_file + 12)? as usize;
    let id_cnt = read_u16(&data, dir_file + 14)? as usize;
    let entries = dir_file + 16;

    for i in 0..(named + id_cnt) {
        let e = entries + i * 8;
        if e + 8 > data.len() {
            break;
        }

        // 根层级跳过命名条目，只看 ID 条目
        if depth == 0 && i < named {
            continue;
        }

        let id = read_u32(&data, e)?;
        let val = read_u32(&data, e + 4)? as usize;

        if depth == 0 && id != 16 {
            continue;
        }

        if val & 0x80000000 != 0 {
            // 子目录：val & 0x7FFFFFFF 是相对于资源节起始的偏移
            let sub = base + (val & 0x7FFFFFFF);
            if let Some(result) = find_version_resource(data, base, sub, depth + 1) {
                return Some(result);
            }
        } else {
            // 数据条目：val 是相对于资源节起始的文件偏移
            return Some(base + val);
        }
    }
    None
}

/// 将 RVA 转换为文件偏移（遍历节表）
fn rva_to_offset(data: &[u8], sections_start: usize, num_sections: usize, rva: usize) -> Option<usize> {
    for i in 0..num_sections {
        let s = sections_start + i * 40;
        if s + 40 > data.len() {
            break;
        }
        let va = read_u32(&data, s + 12)? as usize;
        let vsize = read_u32(&data, s + 8)? as usize;
        let raw = read_u32(&data, s + 20)? as usize;
        if rva >= va && rva < va + vsize {
            return Some(raw + (rva - va));
        }
    }
    None
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    if offset + 2 > data.len() {
        return None;
    }
    Some(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > data.len() {
        return None;
    }
    Some(u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]))
}
