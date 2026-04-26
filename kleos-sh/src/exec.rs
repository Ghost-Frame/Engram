use std::process::Stdio;
use tokio::process::Command;

pub struct ExecResult {
    pub exit_code: i32,
}

pub async fn run_command(command: &str) -> Result<ExecResult, String> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("failed to spawn shell: {}", e))?;

    let status = child
        .wait()
        .await
        .map_err(|e| format!("failed to wait on child: {}", e))?;

    Ok(ExecResult {
        exit_code: status.code().unwrap_or(1),
    })
}
