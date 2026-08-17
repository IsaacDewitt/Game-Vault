use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use super::constants::EXE_VERSION_READ_LIMIT;

/// 文件元数据信息（用于缓存判断）
pub struct FileMetadata {
    /// 最后修改时间（Unix 时间戳秒数）
    pub modified_at: i64,
    /// 文件大小（字节）
    pub file_size: i64,
}

/// 获取文件的元数据（修改时间和大小）
/// 用于判断文件是否发生变化，避免每次都读取 exe 版本
pub fn get_file_metadata(path: &str) -> Option<FileMetadata> {
    let metadata = std::fs::metadata(path).ok()?;

    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let file_size = metadata.len() as i64;

    Some(FileMetadata {
        modified_at,
        file_size,
    })
}

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
/// 支持 %% 表示字面量百分号（如 %%USERPROFILE%% 等同于 %USERPROFILE%）
/// 支持不带 % 的常见环境变量名开头（如 USERPROFILE\Documents\...）
pub fn expand_env_vars(path: &str) -> String {
    // 规范化：去除首尾空白和不可见字符，统一斜杠
    let trimmed: String = path.chars().filter(|c| !c.is_control()).collect();
    let mut result = trimmed.trim().to_string();

    /// 解析环境变量，优先系统环境变量，兜底用 dirs crate
    fn resolve_env_var(var_name: &str) -> Option<String> {
        if let Ok(val) = std::env::var(var_name) {
            if !val.is_empty() {
                return Some(val);
            }
        }
        match var_name {
            "USERPROFILE" | "HOME" => dirs::home_dir().map(|p| p.to_string_lossy().to_string()),
            "APPDATA" => dirs::config_dir().map(|p| p.to_string_lossy().to_string()),
            "LOCALAPPDATA" => dirs::data_local_dir().map(|p| p.to_string_lossy().to_string()),
            "TEMP" | "TMP" => Some(std::env::temp_dir().to_string_lossy().to_string()),
            "PUBLIC" => std::env::var("PUBLIC").ok().filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    // 第一步：把 %%VARNAME%% 规范化为 %VARNAME%
    // 注意：必须按 str 切片处理而非逐字节，否则会破坏中文等多字节 UTF-8 字符
    {
        let mut normalized = String::with_capacity(result.len());
        let mut rest = result.as_str();
        while let Some(pos) = rest.find("%%") {
            normalized.push_str(&rest[..pos]);
            let after = &rest[pos + 2..];
            if let Some(end_rel) = after.find("%%") {
                let var_name = &after[..end_rel];
                // 确保变量名非空且不含 %（避免误匹配）
                if !var_name.is_empty() && !var_name.contains('%') {
                    normalized.push('%');
                    normalized.push_str(var_name);
                    normalized.push('%');
                    rest = &after[end_rel + 2..]; // 跳过整个 %%VARNAME%%
                    continue;
                }
            }
            // 未匹配到闭合 %%，原样保留 "%%"
            normalized.push_str("%%");
            rest = after;
        }
        normalized.push_str(rest);
        result = normalized;
    }

    // 第二步：展开 %VARNAME% 格式
    let mut start = 0;
    while let Some(prefix_pos) = result[start..].find('%') {
        let abs_prefix = start + prefix_pos;
        if let Some(suffix_pos) = result[abs_prefix + 1..].find('%') {
            let abs_suffix = abs_prefix + 1 + suffix_pos;
            let var_name = &result[abs_prefix + 1..abs_suffix];
            if var_name.is_empty() {
                start = abs_suffix + 1;
                continue;
            }
            if let Some(var_value) = resolve_env_var(var_name) {
                result = format!("{}{}{}", &result[..abs_prefix], var_value, &result[abs_suffix + 1..]);
                start = abs_prefix + var_value.len();
            } else {
                start = abs_suffix + 1;
            }
        } else {
            break;
        }
    }

    // 第二步：处理不带 % 的裸变量名（如 "USERPROFILE\Documents\..."）
    // 按长度倒序匹配，避免短名称误匹配长名称前缀
    const COMMON_ENV_VARS: &[&str] = &[
        "CommonProgramFiles(x86)", "ProgramFiles(x86)",
        "USERPROFILE", "LOCALAPPDATA", "CommonProgramFiles",
        "APPDATA", "ProgramFiles", "SystemRoot", "TEMP", "PUBLIC", "TMP",
    ];
    for var_name in COMMON_ENV_VARS {
        if result.len() > var_name.len()
            && result.starts_with(var_name)
            && matches!(result.as_bytes().get(var_name.len()), Some(b'\\') | Some(b'/'))
        {
            if let Some(val) = resolve_env_var(var_name) {
                let after = &result[var_name.len()..];
                result = format!("{}{}", val, after);
            }
            break;
        }
    }

    // 处理 ~ 路径
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
        let s = sections_start.checked_add(i.checked_mul(40)?)?;
        if s.checked_add(40)? > data.len() {
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
