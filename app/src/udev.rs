//! Installing the udev rule that lets the logged-in user access the scanner.
//!
//! Needs root on the host, so it runs through polkit (`pkexec`, which shows the desktop's
//! password prompt). Inside the Flatpak sandbox the command is forwarded to the host with
//! `flatpak-spawn --host` (requires `--talk-name=org.freedesktop.Flatpak`).

use std::io::Write;
use std::process::{Command, Stdio};

pub const RULES: &str = include_str!("../../packaging/60-diascan.rules");
pub const RULES_PATH: &str = "/etc/udev/rules.d/60-diascan.rules";

/// Shell script run as root on the host; the rule text arrives on stdin, so it works even
/// though the host cannot see files inside the sandbox.
const SCRIPT: &str = "set -e
cat > /etc/udev/rules.d/60-diascan.rules
chmod 644 /etc/udev/rules.d/60-diascan.rules
udevadm control --reload-rules
udevadm trigger --subsystem-match=usb --attr-match=idVendor=05e3 --action=add
udevadm settle --timeout=10 || true";

pub fn in_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// Self-contained commands the user can paste into a terminal if automatic installation
/// is not possible (the rule text is embedded, so no file path is needed).
pub fn manual_instructions() -> String {
    let rules: Vec<&str> = RULES.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect();
    format!(
        "sudo tee {RULES_PATH} > /dev/null <<'EOF'\n{}\nEOF\nsudo udevadm control --reload-rules\nsudo udevadm trigger --subsystem-match=usb --attr-match=idVendor=05e3 --action=add",
        rules.join("\n")
    )
}

/// Installs the rule (blocks while the password dialog is open).
pub fn install() -> Result<(), String> {
    let mut cmd = if in_flatpak() {
        let mut c = Command::new("flatpak-spawn");
        c.args(["--host", "pkexec", "/bin/sh", "-c", SCRIPT]);
        c
    } else {
        let mut c = Command::new("pkexec");
        c.args(["/bin/sh", "-c", SCRIPT]);
        c
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound if in_flatpak() => "Could not reach the host system (flatpak-spawn is missing).".to_string(),
            std::io::ErrorKind::NotFound => "pkexec (polkit) is not installed, so DiaScan cannot ask for your password.".to_string(),
            _ => format!("could not start pkexec: {e}"),
        })?;
    child.stdin.take().unwrap().write_all(RULES.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    match out.status.code() {
        Some(0) => Ok(()),
        // pkexec: 126 = authorisation dismissed/denied, 127 = not authorised / no agent
        Some(126) => Err("The password dialog was cancelled.".into()),
        Some(127) => Err("Not authorised (is a polkit authentication agent running?).".into()),
        _ => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(if err.trim().is_empty() { format!("installation failed ({})", out.status) } else { err.trim().to_string() })
        }
    }
}
