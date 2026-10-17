//! The member side: `sesync team join` on a machine, and the stored credential it leaves behind.
//!
//! The credential file holds the member's token for one team. It is written owner-only (mode
//! 600) and read back only by the owner. Nothing here prints the token, and errors never echo
//! its contents.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The stored credential for one team on one machine.
#[derive(Debug, PartialEq)]
pub struct Credential {
    pub space: String,
    pub member: String,
    pub machine: String,
    pub token: String,
}

/// Where the credential for `space` lives: a file under the user's config directory.
pub fn credential_path(config_dir: &Path, space: &str) -> PathBuf {
    config_dir.join("teams").join(format!("{space}.cred"))
}

/// Write the credential owner-only. Creates the directory with owner-only permissions too.
pub fn save(path: &Path, c: &Credential) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|_| "cannot create the team credential directory".to_string())?;
        set_owner_only(dir, 0o700)?;
    }
    let text = format!("space={}\nmember={}\nmachine={}\ntoken={}\n", c.space, c.member, c.machine, c.token);
    let tmp = path.with_extension("cred.tmp");
    {
        let mut f = open_owner_only(&tmp)?;
        f.write_all(text.as_bytes()).map_err(|_| "cannot write the team credential".to_string())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path).map_err(|_| "cannot save the team credential".to_string())
}

/// Read a credential back. Refuses a file that other users can read, since the token is in it.
pub fn load(path: &Path) -> Result<Credential, String> {
    check_owner_only(path)?;
    let text = std::fs::read_to_string(path).map_err(|_| "no team credential for this team; run sesync team join".to_string())?;
    let mut space = None; let mut member = None; let mut machine = None; let mut token = None;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "space" => space = Some(v.to_string()),
                "member" => member = Some(v.to_string()),
                "machine" => machine = Some(v.to_string()),
                "token" => token = Some(v.to_string()),
                _ => {}
            }
        }
    }
    match (space, member, machine, token) {
        (Some(space), Some(member), Some(machine), Some(token)) => Ok(Credential { space, member, machine, token }),
        _ => Err("the team credential file is damaged; run sesync team join again".into()),
    }
}

#[cfg(unix)]
fn open_owner_only(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
        .map_err(|_| "cannot create the team credential".to_string())
}

#[cfg(not(unix))]
fn open_owner_only(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(path)
        .map_err(|_| "cannot create the team credential".to_string())
}

#[cfg(unix)]
fn set_owner_only(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|_| "cannot restrict the team credential directory".to_string())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path, _mode: u32) -> Result<(), String> { Ok(()) }

#[cfg(unix)]
fn check_owner_only(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|_| "no team credential for this team; run sesync team join".to_string())?;
    if meta.permissions().mode() & 0o077 != 0 {
        return Err("the team credential is readable by other users; refusing to use it".into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_owner_only(_path: &Path) -> Result<(), String> { Ok(()) }

/// Join a team on this machine with the 6-digit code. Calls `/enrol`, then saves the token.
/// The code is read by the caller and passed in; it is never stored.
pub fn join(base: &str, space: &str, member: &str, machine: &str, code: &str, config_dir: &Path) -> Result<PathBuf, String> {
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return Err("the code is 6 digits".into());
    }
    let body = serde_json::json!({"space": space, "member": member, "machine": machine, "code": code});
    let reply = post(base, "/enrol", None, body)?;
    let token = reply["token"].as_str().ok_or("the server gave no token")?.to_string();
    let path = credential_path(config_dir, space);
    save(&path, &Credential {
        space: space.into(), member: member.into(), machine: machine.into(), token,
    })?;
    Ok(path)
}

/// Renew the stored token silently, inside the 7-day window. Writes the new token back.
pub fn renew(base: &str, config_dir: &Path, space: &str) -> Result<(), String> {
    let path = credential_path(config_dir, space);
    let mut c = load(&path)?;
    let body = serde_json::json!({"space": space, "machine": c.machine});
    let reply = post(base, "/renew", Some(&c.token), body)?;
    c.token = reply["token"].as_str().ok_or("the server gave no token")?.to_string();
    save(&path, &c)
}

fn post(base: &str, path: &str, token: Option<&str>, body: serde_json::Value) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(15)).build();
    let mut req = agent.post(&format!("{}{path}", base.trim_end_matches('/')));
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    match req.send_json(body) {
        Ok(r) => r.into_json::<serde_json::Value>().map_err(|_| "server reply was not JSON".to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_json::<serde_json::Value>().ok()
                .and_then(|j| j["error"].as_str().map(str::to_string)).unwrap_or_default();
            Err(format!("server refused ({code}): {msg}"))
        }
        Err(_) => Err("cannot reach the team server".into()),
    }
}
