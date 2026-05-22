#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use zip::ZipArchive;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn quiet_command<S: AsRef<std::ffi::OsStr>>(program: S) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

fn write_json_atomic(path: &Path, value: &Value) -> io::Result<()> {
    let text = serde_json::to_string(value)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn extract_year_token(name: &str) -> Option<String> {
    name.split(|c: char| !c.is_ascii_digit())
        .find(|token| token.len() == 4 && token.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_string)
}

#[derive(Serialize, Clone)]
struct InstallResult {
    message: String,
}

#[derive(Serialize, Clone)]
struct PhotoshopInstall {
    name: String,
    version: String,
    path: String,
}

#[derive(Serialize, Clone)]
struct UxpPlugin {
    id: String,
    name: String,
    version: String,
    host_version: String,
    source: String,
    path: String,
}

#[derive(Serialize, Clone)]
struct PsStatus {
    upia_path: Option<String>,
    photoshop_versions: Vec<PhotoshopInstall>,
    installed_uxp: Vec<UxpPlugin>,
}

fn get_upia_path() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let path = "/Library/Application Support/Adobe/Adobe Desktop Common/RemoteComponents/UPI/UnifiedPluginInstallerAgent/UnifiedPluginInstallerAgent.app/Contents/macOS/UnifiedPluginInstallerAgent";
        if std::path::Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    #[cfg(target_os = "windows")]
    {
        let mut roots: Vec<PathBuf> = Vec::new();
        for key in ["CommonProgramFiles", "CommonProgramFiles(x86)", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(v) = std::env::var_os(key) {
                roots.push(PathBuf::from(v));
            }
        }
        roots.push(PathBuf::from(r"C:\Program Files\Common Files"));
        roots.push(PathBuf::from(r"C:\Program Files (x86)\Common Files"));

        for root in roots {
            let dir = root.join("Adobe/Adobe Desktop Common/RemoteComponents/UPI/UnifiedPluginInstallerAgent");
            let direct = dir.join("UnifiedPluginInstallerAgent.exe");
            if direct.exists() {
                return Some(direct.to_string_lossy().to_string());
            }
            for entry in read_dir(&dir) {
                let path = entry.path();
                let ext_is_exe = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("exe"));
                if ext_is_exe && path.is_file() {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

fn read_dir(path: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(path)
        .map(|entries| entries.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

fn plugin_name_from_manifest(manifest: &Value) -> Option<String> {
    manifest
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| manifest.get("displayName").and_then(Value::as_str))
        .map(str::to_string)
}

fn plugin_version_from_manifest(manifest: &Value) -> Option<String> {
    manifest
        .get("version")
        .and_then(Value::as_str)
        .or_else(|| manifest.get("manifestVersion").and_then(Value::as_str))
        .map(str::to_string)
}

fn read_uxp_manifest(plugin_dir: &Path) -> Option<Value> {
    let manifest_path = plugin_dir.join("manifest.json");
    let text = fs::read_to_string(manifest_path).ok()?;
    serde_json::from_str(&text).ok()
}

fn version_from_plugin_folder(folder_name: &str, id: &str) -> Option<String> {
    folder_name
        .strip_prefix(&format!("{}_", id))
        .filter(|version| !version.is_empty())
        .map(str::to_string)
}

fn plugin_id_for_path(path: &Path) -> Option<String> {
    if let Some(manifest) = read_uxp_manifest(path) {
        if let Some(id) = manifest.get("id").and_then(Value::as_str) {
            return Some(id.to_string());
        }
    }

    let folder_name = path.file_name()?.to_str()?;
    folder_name.split('_').next().map(str::to_string)
}

fn normalize_host_min_version(manifest: &mut Value) {
    let Some(host) = manifest.get_mut("host") else {
        return;
    };

    let hosts: Vec<&mut Value> = if let Some(array) = host.as_array_mut() {
        array.iter_mut().collect()
    } else {
        vec![host]
    };

    for host in hosts {
        let Some(min_version) = host.get_mut("minVersion") else {
            continue;
        };
        let Some(version) = min_version.as_str() else {
            continue;
        };
        let mut parts = version.split('.');
        let Some(major) = parts.next() else {
            continue;
        };
        let Some(minor) = parts.next() else {
            continue;
        };
        *min_version = Value::String(format!("{}.{}", major, minor));
    }
}

fn host_min_version_from_manifest(manifest: &Value) -> Option<String> {
    let host = manifest.get("host")?;
    let first = if let Some(array) = host.as_array() {
        array.iter().find(|h| {
            h.get("app")
                .and_then(Value::as_str)
                .is_some_and(|app| app.eq_ignore_ascii_case("PS"))
        }).or_else(|| array.first())?
    } else {
        host
    };
    first
        .get("minVersion")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn ccx_metadata(path: &Path) -> Result<(String, String, String, Value), String> {
    let file = fs::File::open(path).map_err(|e| format!("读取 CCX 失败: {}", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("CCX 不是有效 zip 包: {}", e))?;
    let mut manifest_file = archive
        .by_name("manifest.json")
        .map_err(|_| "CCX 缺少 manifest.json。".to_string())?;
    let mut manifest_text = String::new();
    io::Read::read_to_string(&mut manifest_file, &mut manifest_text)
        .map_err(|e| format!("读取 manifest.json 失败: {}", e))?;
    let mut manifest: Value = serde_json::from_str(&manifest_text)
        .map_err(|e| format!("manifest.json 不是合法 JSON: {}", e))?;
    normalize_host_min_version(&mut manifest);

    let id = manifest
        .get("id")
        .and_then(Value::as_str)
        .ok_or("manifest.json 缺少 id。".to_string())?
        .to_string();
    let name = plugin_name_from_manifest(&manifest).unwrap_or_else(|| id.clone());
    let version = plugin_version_from_manifest(&manifest).unwrap_or_else(|| "0.0".to_string());

    Ok((id, name, version, manifest))
}

fn write_plugins_info_entry(
    root: &Path,
    id: &str,
    name: &str,
    version: &str,
    install_dir_name: &str,
    host_min_version: &str,
) {
    let info_dir = root.join("PluginsInfo/v1");
    let _ = fs::create_dir_all(&info_dir);
    let info_path = info_dir.join("PS.json");
    let mut json = fs::read_to_string(&info_path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({ "plugins": [] }));

    if !json.get("plugins").is_some_and(Value::is_array) {
        json["plugins"] = Value::Array(Vec::new());
    }

    if let Some(plugins) = json.get_mut("plugins").and_then(Value::as_array_mut) {
        plugins.retain(|plugin| plugin.get("pluginId").and_then(Value::as_str) != Some(id));
        plugins.push(serde_json::json!({
            "hostMinVersion": host_min_version,
            "name": name,
            "path": format!("$localPlugins/External/{}", install_dir_name),
            "pluginId": id,
            "status": "enabled",
            "type": "uxp",
            "versionString": version,
        }));
    }

    let _ = write_json_atomic(&info_path, &json);
}

fn install_ccx_locally(
    path: &Path,
    id: &str,
    name: &str,
    version: &str,
    manifest: &Value,
) -> Result<PathBuf, String> {
    let root = user_uxp_root()
        .filter(|root| root.exists() || fs::create_dir_all(root).is_ok())
        .or_else(|| uxp_roots().into_iter().next())
        .ok_or("未找到 Adobe UXP 目录。".to_string())?;
    let install_dir_name = format!("{}_{}", id, version);
    let install_dir = root.join("Plugins/External").join(&install_dir_name);

    remove_third_party_uxp_files(id)?;
    fs::create_dir_all(&install_dir).map_err(|e| format!("创建插件目录失败: {}", e))?;

    let file = fs::File::open(path).map_err(|e| format!("读取 CCX 失败: {}", e))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("CCX 不是有效 zip 包: {}", e))?;
    archive
        .extract(&install_dir)
        .map_err(|e| format!("解压 CCX 失败: {}", e))?;

    fs::write(
        install_dir.join("manifest.json"),
        serde_json::to_string_pretty(manifest).map_err(|e| format!("写入 manifest 失败: {}", e))?,
    )
    .map_err(|e| format!("写入 manifest 失败: {}", e))?;

    let host_min_version =
        host_min_version_from_manifest(manifest).unwrap_or_else(|| "23.0".to_string());
    write_plugins_info_entry(&root, id, name, version, &install_dir_name, &host_min_version);

    Ok(install_dir)
}

fn add_uxp_plugin(
    plugins: &mut BTreeMap<String, UxpPlugin>,
    id: String,
    name: String,
    version: String,
    host_version: String,
    source: &str,
    path: PathBuf,
) {
    let key = format!("{}|{}|{}", source, host_version, id);
    plugins.insert(
        key,
        UxpPlugin {
            id,
            name,
            version,
            host_version,
            source: source.to_string(),
            path: path.to_string_lossy().to_string(),
        },
    );
}

#[cfg(target_os = "macos")]
fn plist_value(path: &Path, key: &str) -> Option<String> {
    let output = quiet_command("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg(format!("Print :{}", key))
        .arg(path)
        .output()
        .ok()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn get_photoshop_installs() -> Vec<PhotoshopInstall> {
    let mut installs = Vec::new();
    let applications = Path::new("/Applications");

    for entry in read_dir(applications) {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.starts_with("Adobe Photoshop") {
            continue;
        }

        let app_path = path.join(format!("{}.app", file_name));
        let info_path = app_path.join("Contents/Info.plist");
        if !info_path.exists() {
            continue;
        }

        let name = plist_value(&info_path, "CFBundleName").unwrap_or_else(|| file_name.to_string());
        let version = plist_value(&info_path, "CFBundleShortVersionString")
            .or_else(|| plist_value(&info_path, "CFBundleVersion"))
            .unwrap_or_else(|| "未知版本".to_string());

        installs.push(PhotoshopInstall {
            name,
            version,
            path: app_path.to_string_lossy().to_string(),
        });
    }

    installs.sort_by(|a, b| b.version.cmp(&a.version));
    installs
}

#[cfg(target_os = "windows")]
fn registry_photoshop_installs() -> Vec<(String, PathBuf)> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    use winreg::RegKey;

    let mut out = Vec::new();
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        let Ok(root) = hklm.open_subkey_with_flags("SOFTWARE\\Adobe\\Photoshop", KEY_READ | view)
        else {
            continue;
        };
        for ver in root.enum_keys().flatten() {
            let Ok(ver_key) = root.open_subkey_with_flags(&ver, KEY_READ | view) else {
                continue;
            };
            let app_path: String = match ver_key.get_value("ApplicationPath") {
                Ok(v) => v,
                Err(_) => continue,
            };
            out.push((ver, PathBuf::from(app_path)));
        }
    }
    out
}

#[cfg(target_os = "windows")]
fn get_photoshop_installs() -> Vec<PhotoshopInstall> {
    let mut installs: Vec<PhotoshopInstall> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut push = |name: String, version: String, exe_path: PathBuf| {
        let key = exe_path.to_string_lossy().to_string();
        if seen.insert(key.clone()) {
            installs.push(PhotoshopInstall {
                name,
                version,
                path: key,
            });
        }
    };

    for (registry_version, dir) in registry_photoshop_installs() {
        let trimmed = dir.to_string_lossy().trim_end_matches('\\').to_string();
        let dir = PathBuf::from(trimmed);
        let exe = dir.join("Photoshop.exe");
        let exe_path = if exe.exists() { exe } else { dir.clone() };
        let folder_name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Adobe Photoshop")
            .to_string();
        let version = extract_year_token(&folder_name).unwrap_or(registry_version);
        push(folder_name, version, exe_path);
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(v) = std::env::var_os(key) {
            roots.push(PathBuf::from(v));
        }
    }
    roots.push(PathBuf::from(r"C:\Program Files"));
    roots.push(PathBuf::from(r"C:\Program Files (x86)"));

    for root in roots {
        let adobe = root.join("Adobe");
        for entry in read_dir(&adobe) {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !file_name.starts_with("Adobe Photoshop") {
                continue;
            }

            let exe = path.join("Photoshop.exe");
            let exe_path = if exe.exists() { exe } else { path.clone() };
            let version = extract_year_token(file_name).unwrap_or_else(|| "未知版本".to_string());

            push(file_name.to_string(), version, exe_path);
        }
    }

    installs.sort_by(|a, b| b.version.cmp(&a.version));
    installs
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn get_photoshop_installs() -> Vec<PhotoshopInstall> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn uxp_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Library/Application Support/Adobe/UXP")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(Path::new(&home).join("Library/Application Support/Adobe/UXP"));
    }
    roots
}

#[cfg(target_os = "macos")]
fn user_uxp_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| Path::new(&home).join("Library/Application Support/Adobe/UXP"))
}

#[cfg(target_os = "windows")]
fn uxp_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(program_data) = std::env::var_os("ProgramData") {
        roots.push(Path::new(&program_data).join("Adobe/UXP"));
    }
    if let Some(app_data) = std::env::var_os("APPDATA") {
        roots.push(Path::new(&app_data).join("Adobe/UXP"));
    }
    roots
}

#[cfg(target_os = "windows")]
fn user_uxp_root() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|app_data| Path::new(&app_data).join("Adobe/UXP"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn uxp_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn user_uxp_root() -> Option<PathBuf> {
    None
}

fn get_installed_uxp_plugins() -> Vec<UxpPlugin> {
    let mut plugins = BTreeMap::new();

    for root in uxp_roots() {
        let local_plugins = root.join("Plugins/External");
        for entry in read_dir(&local_plugins) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let folder_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("未知插件")
                .to_string();
            let manifest = read_uxp_manifest(&path);
            let id = manifest
                .as_ref()
                .and_then(|value| value.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| folder_name.split('_').next().unwrap_or(&folder_name).to_string());
            let name = manifest
                .as_ref()
                .and_then(plugin_name_from_manifest)
                .unwrap_or_else(|| id.clone());
            let version = manifest
                .as_ref()
                .and_then(plugin_version_from_manifest)
                .or_else(|| version_from_plugin_folder(&folder_name, &id))
                .unwrap_or_else(|| "未知版本".to_string());

            add_uxp_plugin(
                &mut plugins,
                id,
                name,
                version,
                "已安装".to_string(),
                "Plugins/External",
                path,
            );
        }

        let extensions = root.join("extensions");
        for entry in read_dir(&extensions) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let folder_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("未知插件")
                .to_string();
            let manifest = read_uxp_manifest(&path);
            let id = manifest
                .as_ref()
                .and_then(|value| value.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| folder_name.clone());
            let name = manifest
                .as_ref()
                .and_then(plugin_name_from_manifest)
                .unwrap_or_else(|| folder_name.clone());
            let version = manifest
                .as_ref()
                .and_then(plugin_version_from_manifest)
                .unwrap_or_else(|| "未知版本".to_string());

            add_uxp_plugin(
                &mut plugins,
                id,
                name,
                version,
                "全局".to_string(),
                "extensions",
                path,
            );
        }

        let plugin_storage = root.join("PluginsStorage/PHSP");
        for host_entry in read_dir(&plugin_storage) {
            let host_path = host_entry.path();
            if !host_path.is_dir() {
                continue;
            }
            let host_version = host_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("未知")
                .to_string();

            for source in ["Internal"] {
                for plugin_entry in read_dir(&host_path.join(source)) {
                    let plugin_path = plugin_entry.path();
                    if !plugin_path.is_dir() {
                        continue;
                    }
                    let id = plugin_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("未知插件")
                        .to_string();
                    let manifest = read_uxp_manifest(&plugin_path.join("PluginData"));
                    let name = manifest
                        .as_ref()
                        .and_then(plugin_name_from_manifest)
                        .unwrap_or_else(|| id.clone());
                    let version = manifest
                        .as_ref()
                        .and_then(plugin_version_from_manifest)
                        .unwrap_or_else(|| "未知版本".to_string());

                    add_uxp_plugin(
                        &mut plugins,
                        id,
                        name,
                        version,
                        host_version.clone(),
                        source,
                        plugin_path,
                    );
                }
            }
        }
    }

    plugins.into_values().collect()
}

fn is_third_party_uxp(plugin: &UxpPlugin) -> bool {
    plugin.source != "Internal" && !plugin.id.starts_with("com.adobe.")
}

fn find_third_party_uxp(id: &str, host_version: &str, source: &str) -> Option<UxpPlugin> {
    get_installed_uxp_plugins()
        .into_iter()
        .find(|plugin| {
            plugin.id == id
                && plugin.host_version == host_version
                && plugin.source == source
                && is_third_party_uxp(plugin)
        })
}

fn remove_plugins_info_entry(root: &Path, id: &str) {
    let info_path = root.join("PluginsInfo/v1/PS.json");
    let Ok(text) = fs::read_to_string(&info_path) else {
        return;
    };
    let Ok(mut json) = serde_json::from_str::<Value>(&text) else {
        return;
    };

    let Some(plugins) = json.get_mut("plugins").and_then(Value::as_array_mut) else {
        return;
    };
    let before = plugins.len();
    plugins.retain(|plugin| plugin.get("pluginId").and_then(Value::as_str) != Some(id));

    if plugins.len() != before {
        let _ = write_json_atomic(&info_path, &json);
    }
}

fn map_remove_error(action: &str, err: io::Error) -> String {
    let locked = matches!(err.raw_os_error(), Some(32) | Some(33))
        || err.kind() == io::ErrorKind::PermissionDenied;
    if locked {
        format!(
            "{}失败: 文件被占用。请先完全关闭 Photoshop（包括系统托盘图标）后重试。\n详细: {}",
            action, err
        )
    } else {
        format!("{}失败: {}", action, err)
    }
}

fn remove_third_party_uxp_files(id: &str) -> Result<usize, String> {
    let mut removed = 0;

    for root in uxp_roots() {
        for entry in read_dir(&root.join("Plugins/External")) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if plugin_id_for_path(&path).as_deref() == Some(id) {
                fs::remove_dir_all(&path).map_err(|e| map_remove_error("删除旧版本", e))?;
                removed += 1;
            }
        }

        let plugin_storage = root.join("PluginsStorage/PHSP");
        for host_entry in read_dir(&plugin_storage) {
            let cache_path = host_entry.path().join("External").join(id);
            if cache_path.exists() {
                fs::remove_dir_all(&cache_path).map_err(|e| map_remove_error("清理缓存", e))?;
                removed += 1;
            }
        }

        remove_plugins_info_entry(&root, id);
    }

    Ok(removed)
}

#[tauri::command]
fn check_upia() -> Result<String, String> {
    match get_upia_path() {
        Some(p) => Ok(p),
        None => Err("未找到 Adobe 插件安装器。请确保已安装 Photoshop 2022+。".to_string()),
    }
}

#[tauri::command]
fn get_ps_status() -> PsStatus {
    PsStatus {
        upia_path: get_upia_path(),
        photoshop_versions: get_photoshop_installs(),
        installed_uxp: get_installed_uxp_plugins(),
    }
}

#[tauri::command]
fn install_ccx(path: String) -> Result<InstallResult, String> {
    let ccx_path = Path::new(&path);

    if !ccx_path.exists() {
        return Err("文件不存在。".to_string());
    }

    let (plugin_id, plugin_name, plugin_version, manifest) = ccx_metadata(ccx_path)?;

    let install_dir =
        install_ccx_locally(ccx_path, &plugin_id, &plugin_name, &plugin_version, &manifest)?;

    Ok(InstallResult {
        message: format!(
            "已安装 {} {}\n位置: {}\n\n请完全关闭并重启 Photoshop（包括右下角托盘图标），插件菜单里就能看到。",
            plugin_name,
            plugin_version,
            install_dir.display()
        ),
    })
}

#[tauri::command]
fn uninstall_uxp(id: String, host_version: String, source: String) -> Result<InstallResult, String> {
    let plugin = find_third_party_uxp(&id, &host_version, &source)
        .ok_or("只能卸载第三方 UXP 插件。".to_string())?;
    remove_third_party_uxp_files(&plugin.id)?;

    Ok(InstallResult {
        message: format!("已卸载 {}。请重启 Photoshop。", plugin.name),
    })
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            check_upia,
            get_ps_status,
            install_ccx,
            uninstall_uxp
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
