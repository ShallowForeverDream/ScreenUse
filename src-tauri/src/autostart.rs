use anyhow::{anyhow, Context, Result};

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ScreenUse";

pub fn background_launch_requested() -> bool {
    std::env::args().any(|argument| argument == "--background")
}

#[cfg(windows)]
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    if enabled {
        let executable = std::env::current_exe().context("cannot locate ScreenUse executable")?;
        let value = launch_command(&executable);
        let output = hidden_reg_command()
            .args(["ADD", RUN_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d"])
            .arg(value)
            .args(["/f"])
            .output()
            .context("cannot update Windows login startup")?;
        if !output.status.success() {
            return Err(command_error(
                "enable Windows login startup",
                &output.stderr,
            ));
        }
        return Ok(());
    }

    let output = hidden_reg_command()
        .args(["DELETE", RUN_KEY, "/v", VALUE_NAME, "/f"])
        .output()
        .context("cannot update Windows login startup")?;
    if output.status.success() || !startup_value_exists() {
        Ok(())
    } else {
        Err(command_error(
            "disable Windows login startup",
            &output.stderr,
        ))
    }
}

#[cfg(not(windows))]
pub fn set_launch_at_login(_enabled: bool) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn startup_value_exists() -> bool {
    hidden_reg_command()
        .args(["QUERY", RUN_KEY, "/v", VALUE_NAME])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn hidden_reg_command() -> std::process::Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new("reg.exe");
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(windows)]
fn launch_command(executable: &std::path::Path) -> String {
    format!("\"{}\" --background", executable.display())
}

#[cfg(windows)]
fn command_error(action: &str, stderr: &[u8]) -> anyhow::Error {
    let details = String::from_utf8_lossy(stderr).trim().to_string();
    if details.is_empty() {
        anyhow!("failed to {action}")
    } else {
        anyhow!("failed to {action}: {details}")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn quotes_executable_for_background_launch() {
        let command = launch_command(std::path::Path::new(
            r"C:\Program Files\ScreenUse\screenuse.exe",
        ));
        assert_eq!(
            command,
            r#""C:\Program Files\ScreenUse\screenuse.exe" --background"#
        );
    }
}
