
use crate::i18n::{tr, trf};
use crate::store::{atomic_write, effective_zcode_path, now_ts, Paths};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[cfg(windows)]
fn no_window(prog: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(prog);
    c.creation_flags(0x0800_0000);
    c
}
#[cfg(not(windows))]
fn no_window(prog: &str) -> Command {
    Command::new(prog)
}

#[cfg(windows)]
fn detached(mut c: Command) -> Command {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0000_0008 | 0x0000_0200);
    c
}
#[cfg(not(windows))]
fn detached(mut c: Command) -> Command {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    c.process_group(0);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c
}

/// 一个多开实例档案：独立数据目录 + 绑定账号 + 可选端口 + 可选独立客户端路径。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// ZCode 数据根（ZCode 将在此目录下读写 .zcode/v2/*）
    pub data_root: String,
    #[serde(default)]
    pub zcode_path: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub account_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn profiles_dir(paths: &Paths) -> PathBuf {
    paths.store_dir().join("profiles")
}

fn profiles_file(paths: &Paths) -> PathBuf {
    profiles_dir(paths).join("profiles.json")
}

fn pid_file(paths: &Paths, id: &str) -> PathBuf {
    profiles_dir(paths).join(format!("{id}.pid"))
}

pub fn valid_profile_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn list_profiles(paths: &Paths) -> Vec<Profile> {
    let raw = match fs::read_to_string(profiles_file(paths)) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    serde_json::from_str::<Vec<Profile>>(&raw).unwrap_or_default()
}

pub fn save_profiles(paths: &Paths, profiles: &[Profile]) -> Result<(), String> {
    fs::create_dir_all(profiles_dir(paths))
        .map_err(|e| trf("err.mkdir", &[("e", &e.to_string())]))?;
    let body = serde_json::to_string_pretty(profiles).unwrap_or_default() + "\n";
    atomic_write(&profiles_file(paths), &body)
}

pub fn get_profile(paths: &Paths, id: &str) -> Result<Profile, String> {
    list_profiles(paths)
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| trf("err.profile.not_found", &[("id", id)]))
}

fn data_root_conflict(profiles: &[Profile], self_id: Option<&str>, data_root: &Path) -> bool {
    let norm = |p: &Path| norm_path(p);
    let target = norm(data_root);
    profiles
        .iter()
        .any(|p| Some(p.id.as_str()) != self_id && norm(Path::new(&p.data_root)) == target)
}

fn norm_path(p: &Path) -> String {
    let s = p.to_string_lossy().to_lowercase().replace('\\', "/");
    s.trim_end_matches('/').to_string()
}

/// 同一账号禁止绑到多个 Profile：refresh token 轮换会让双实例互相踢下线（服务端单活跃会话）。
fn account_bound_elsewhere(profiles: &[Profile], self_id: Option<&str>, account_id: &str) -> bool {
    profiles.iter().any(|p| {
        Some(p.id.as_str()) != self_id && p.account_id.as_deref() == Some(account_id)
    })
}

pub fn create_profile(
    paths: &Paths,
    name: &str,
    data_root: &str,
    port: Option<u16>,
    account_id: Option<String>,
    zcode_path: Option<String>,
) -> Result<Profile, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(tr("err.profile.name_empty"));
    }
    let root = PathBuf::from(data_root.trim());
    if data_root.trim().is_empty() || !root.is_absolute() {
        return Err(tr("err.profile.data_root_abs"));
    }
    let mut profiles = list_profiles(paths);
    if data_root_conflict(&profiles, None, &root) {
        return Err(tr("err.profile.data_root_taken"));
    }
    if let Some(acc) = account_id.as_deref().filter(|s| !s.trim().is_empty()) {
        if account_bound_elsewhere(&profiles, None, acc) {
            return Err(trf("err.profile.account_bound", &[("id", acc)]));
        }
    }
    // 数据根不得与默认实例（不走 Profile 的主实例）重叠
    if norm_path(&root) == norm_path(&paths.data_root) {
        return Err(tr("err.profile.data_root_default"));
    }
    let ts = now_ts();
    let p = Profile {
        id: Uuid::new_v4().simple().to_string(),
        name: name.to_string(),
        data_root: root.to_string_lossy().to_string(),
        zcode_path: zcode_path.filter(|s| !s.trim().is_empty()),
        port,
        account_id: account_id.filter(|s| !s.trim().is_empty()),
        created_at: ts.clone(),
        updated_at: ts,
    };
    profiles.push(p.clone());
    save_profiles(paths, &profiles)?;
    Ok(p)
}

pub fn update_profile(
    paths: &Paths,
    id: &str,
    name: Option<String>,
    data_root: Option<String>,
    port: Option<Option<u16>>,
    account_id: Option<Option<String>>,
    zcode_path: Option<Option<String>>,
) -> Result<Profile, String> {
    let mut profiles = list_profiles(paths);
    let Some(idx) = profiles.iter().position(|p| p.id == id) else {
        return Err(trf("err.profile.not_found", &[("id", id)]));
    };
    let mut p = profiles[idx].clone();
    if let Some(n) = name {
        let n = n.trim();
        if n.is_empty() {
            return Err(tr("err.profile.name_empty"));
        }
        p.name = n.to_string();
    }
    if let Some(dr) = data_root {
        let root = PathBuf::from(dr.trim());
        if dr.trim().is_empty() || !root.is_absolute() {
            return Err(tr("err.profile.data_root_abs"));
        }
        if data_root_conflict(&profiles, Some(id), &root) {
            return Err(tr("err.profile.data_root_taken"));
        }
        if norm_path(&root) == norm_path(&paths.data_root) {
            return Err(tr("err.profile.data_root_default"));
        }
        p.data_root = root.to_string_lossy().to_string();
    }
    if let Some(port) = port {
        p.port = port;
    }
    if let Some(acc) = account_id {
        if let Some(a) = acc.as_deref().filter(|s| !s.trim().is_empty()) {
            if account_bound_elsewhere(&profiles, Some(id), a) {
                return Err(trf("err.profile.account_bound", &[("id", a)]));
            }
        }
        p.account_id = acc.filter(|s| !s.trim().is_empty());
    }
    if let Some(zp) = zcode_path {
        p.zcode_path = zp.filter(|s| !s.trim().is_empty());
    }
    p.updated_at = now_ts();
    profiles[idx] = p.clone();
    save_profiles(paths, &profiles)?;
    Ok(p)
}

pub fn delete_profile(paths: &Paths, id: &str) -> Result<(), String> {
    let p = get_profile(paths, id)?;
    if profile_running(paths, &p) {
        return Err(tr("err.profile.running"));
    }
    let profiles: Vec<Profile> = list_profiles(paths).into_iter().filter(|x| x.id != id).collect();
    if profiles.len() == list_profiles(paths).len() {
        return Err(trf("err.profile.not_found", &[("id", id)]));
    }
    save_profiles(paths, &profiles)?;
    let _ = fs::remove_file(pid_file(paths, id));
    Ok(())
}

/// Profile 专属 Paths：home 不变（账号库/加密密钥一致），data_root 指向该实例的数据目录。
pub fn paths_for_profile(paths: &Paths, p: &Profile) -> Paths {
    Paths {
        home: paths.home.clone(),
        data_root: PathBuf::from(&p.data_root),
    }
}

pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        no_window("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.split(',').nth(1).map(|f| f.trim().trim_matches('"')) == Some(&pid.to_string()))
            })
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        // kill(pid, 0) 仅探测存活
        let pid_i = pid as i32;
        libc_kill_probe(pid_i)
    }
}

#[cfg(not(windows))]
fn libc_kill_probe(pid: i32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // sig=0: 不发信号，仅检查进程是否存在
    unsafe { kill(pid, 0) == 0 }
}

pub fn profile_running(paths: &Paths, p: &Profile) -> bool {
    let Ok(raw) = fs::read_to_string(pid_file(paths, &p.id)) else {
        return false;
    };
    let pid: u32 = match raw.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            let _ = fs::remove_file(pid_file(paths, &p.id));
            return false;
        }
    };
    if pid_alive(pid) {
        return true;
    }
    let _ = fs::remove_file(pid_file(paths, &p.id));
    false
}

fn write_pid(paths: &Paths, id: &str, pid: u32) -> Result<(), String> {
    fs::create_dir_all(profiles_dir(paths))
        .map_err(|e| trf("err.mkdir", &[("e", &e.to_string())]))?;
    atomic_write(&pid_file(paths, id), &format!("{pid}\n"))
}

fn resolve_profile_zcode_path(paths: &Paths, p: &Profile) -> Result<String, String> {
    if let Some(zp) = p.zcode_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if Path::new(zp).exists() {
            return Ok(zp.to_string());
        }
        return Err(trf("err.zcode.path_invalid", &[("p", zp)]));
    }
    let (p, ok) = effective_zcode_path(paths);
    if !ok {
        return Err(trf("err.zcode.path_invalid", &[("p", &p)]));
    }
    Ok(p)
}

fn wait_gone(pid: u32, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    !pid_alive(pid)
}

/// 以 Profile 启动一个独立 ZCode 实例：注入 ZCODE_DATA_BASE_DIR（及可选 ZCODE_PORT），记录 PID。
pub fn launch_profile(paths: &Paths, p: &Profile) -> Result<u32, String> {
    if profile_running(paths, p) {
        return Err(trf("err.profile.already_running", &[("name", &p.name)]));
    }
    let bin = resolve_profile_zcode_path(paths, p)?;
    let root = PathBuf::from(&p.data_root);
    fs::create_dir_all(root.join(".zcode").join("v2"))
        .map_err(|e| trf("err.mkdir", &[("e", &e.to_string())]))?;
    let mut cmd = detached(no_window(&bin));
    // ZCode 的数据根重定向入口；若客户端 home 的 setting.json 含 dataBaseDir，优先级以客户端实现为准
    cmd.env("ZCODE_DATA_BASE_DIR", &root);
    // Electron userData 隔离：SingletonLock、rum-electron-store(_arms_uid)、会话数据随实例独立
    // （app.asar 证实：userData = ZCODE_DESKTOP_USER_DATA_DIR ?? appData/<应用名>）
    let user_data = root.join("user-data");
    cmd.env("ZCODE_DESKTOP_USER_DATA_DIR", &user_data);
    cmd.env("ZCODE_DESKTOP_SESSION_DATA_DIR", user_data.join("session"));
    if let Some(port) = p.port {
        cmd.env("ZCODE_PORT", port.to_string());
    }
    let child = cmd
        .spawn()
        .map_err(|e| trf("err.zcode.launch", &[("e", &e.to_string())]))?;
    let pid = child.id();
    write_pid(paths, &p.id, pid)?;
    Ok(pid)
}

/// 终止该 Profile 记录的 ZCode 实例（只杀 PID，不影响其他实例）。
pub fn kill_profile(paths: &Paths, p: &Profile) -> Result<bool, String> {
    let raw = match fs::read_to_string(pid_file(paths, &p.id)) {
        Ok(r) => r,
        Err(_) => return Ok(true),
    };
    let pid: u32 = match raw.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            let _ = fs::remove_file(pid_file(paths, &p.id));
            return Ok(true);
        }
    };
    #[cfg(windows)]
    let killed = no_window("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output()
        .is_ok();
    #[cfg(not(windows))]
    let killed = {
        let pid_i = pid as i32;
        unsafe {
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            kill(pid_i, 15) == 0
        }
    };
    if !killed && !pid_alive(pid) {
        let _ = fs::remove_file(pid_file(paths, &p.id));
        return Ok(true);
    }
    let gone = wait_gone(pid, 8);
    if !gone {
        #[cfg(not(windows))]
        unsafe {
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            let _ = kill(pid as i32, 9);
        }
        #[cfg(windows)]
        {
            let _ = no_window("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
        let gone = wait_gone(pid, 4);
        if !gone {
            return Err(tr("err.profile.kill_timeout"));
        }
    }
    let _ = fs::remove_file(pid_file(paths, &p.id));
    Ok(true)
}

pub fn profile_value(paths: &Paths, p: &Profile) -> Value {
    let running = profile_running(paths, p);
    let pid = fs::read_to_string(pid_file(paths, &p.id))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|_| running);
    let account_name = p
        .account_id
        .as_deref()
        .and_then(|id| crate::store::load_account(paths, id).ok())
        .map(|a| a.name);
    json!({
        "id": p.id,
        "name": p.name,
        "dataRoot": p.data_root,
        "zcodePath": p.zcode_path,
        "port": p.port,
        "accountId": p.account_id,
        "accountName": account_name,
        "running": running,
        "pid": pid,
        "createdAt": p.created_at,
        "updatedAt": p.updated_at,
    })
}

pub fn list_profiles_value(paths: &Paths) -> Value {
    json!({
        "profiles": list_profiles(paths)
            .iter()
            .map(|p| profile_value(paths, p))
            .collect::<Vec<_>>(),
    })
}
