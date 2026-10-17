//! `sesync team ...`: the admin commands for teams, run on the operator's machine.
//!
//!   sesync team create <name> [--member <m>]...   create a team; prints the authenticator URI once
//!   sesync team invite <name> <member>           add a member to an existing team
//!
//! Settings come from the environment, never the command line, so nothing secret is in shell
//! history or the process list:
//!   SESYNC_TEAM_URL          base URL of the team server, e.g. http://127.0.0.1:30500
//!   SESYNC_ADMIN_TOKEN_FILE  path to the admin token file (mode 600, owner only)
//!
//! The admin token is read from its file and sent only in the Authorization header. It is
//! never printed, logged or echoed in an error.

use std::path::Path;

#[derive(Debug, PartialEq)]
pub enum Command {
    Create { name: String, members: Vec<String> },
    Invite { name: String, member: String },
}

/// Parse the arguments after `team`. Returns a usage message on any mistake.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let usage = "usage: sesync team create <name> [--member <m>]... | sesync team invite <name> <member>";
    match args.first().map(String::as_str) {
        Some("create") => {
            let name = args.get(1).filter(|n| !n.starts_with("--")).ok_or(usage)?.clone();
            let mut members = Vec::new();
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--member" {
                    let m = args.get(i + 1).ok_or(usage)?.clone();
                    members.push(m);
                    i += 2;
                } else {
                    return Err(usage.into());
                }
            }
            Ok(Command::Create { name, members })
        }
        Some("invite") => {
            let name = args.get(1).ok_or(usage)?.clone();
            let member = args.get(2).ok_or(usage)?.clone();
            if args.len() != 3 { return Err(usage.into()); }
            Ok(Command::Invite { name, member })
        }
        _ => Err(usage.into()),
    }
}

/// Read the admin token from a file that only its owner can read. Refuses a file that other
/// users can read, since the token would then be exposed.
pub fn read_admin_token(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|_| "cannot read the admin token file".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err("admin token file must be mode 600 (owner only)".into());
        }
    }
    let _ = meta;
    let token = std::fs::read_to_string(path).map_err(|_| "cannot read the admin token file".to_string())?;
    let token = token.trim().to_string();
    if token.len() < 32 {
        return Err("admin token is too short".into());
    }
    Ok(token)
}

/// Run a parsed command against the team server. Returns the text to print.
pub fn run(cmd: &Command, base: &str, token: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(15)).build();
    let base = base.trim_end_matches('/');
    let auth = format!("Bearer {token}");
    match cmd {
        Command::Create { name, members } => {
            let body = serde_json::json!({"space": name, "members": members});
            let resp = call(&agent, &format!("{base}/admin/team"), &auth, body)?;
            let uri = resp["otpauth_uri"].as_str().ok_or("server gave no authenticator URI")?;
            Ok(format!(
                "team '{name}' created.\n\
                 Add it to your authenticator app NOW. It is shown once and cannot be read back:\n\
                 {uri}\n"))
        }
        Command::Invite { name, member } => {
            let body = serde_json::json!({"space": name, "member": member});
            call(&agent, &format!("{base}/admin/invite"), &auth, body)?;
            Ok(format!("invited '{member}' to team '{name}'."))
        }
    }
}

fn call(agent: &ureq::Agent, url: &str, auth: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    match agent.post(url).set("Authorization", auth).send_json(body) {
        Ok(r) => r.into_json::<serde_json::Value>().map_err(|_| "server reply was not JSON".to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_json::<serde_json::Value>().ok()
                .and_then(|j| j["error"].as_str().map(str::to_string))
                .unwrap_or_default();
            Err(format!("server refused ({code}): {msg}"))
        }
        Err(_) => Err("cannot reach the team server".into()),
    }
}
