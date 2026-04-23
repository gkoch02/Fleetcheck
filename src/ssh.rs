use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use openssh::{KnownHosts, SessionBuilder, Stdio};
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

use crate::config::HostConfig;

/// The bundled metric-collection script. Embedded at compile time so the
/// binary has no runtime dependency on anything other than `ssh`.
const SCRIPT: &str = include_str!("script.sh");

/// Open a multiplexed SSH session to `host`. The Session holds a running
/// ControlMaster; subsequent `session.command(...)` calls reuse its socket
/// rather than re-doing TCP + auth.
///
/// `connect_timeout` bounds the TCP/handshake phase specifically, which is
/// what we want for "is this host reachable?". Command execution is timed
/// out separately by the caller.
pub async fn connect(
    name: &str,
    host: &HostConfig,
    connect_timeout: Duration,
) -> Result<openssh::Session> {
    let destination = host.addr.as_deref().unwrap_or(name);

    let mut builder = SessionBuilder::default();
    builder
        .known_hosts_check(KnownHosts::Strict)
        .connect_timeout(connect_timeout);
    if let Some(user) = &host.user {
        builder.user(user.clone());
    }
    if let Some(port) = host.port {
        builder.port(port);
    }

    builder
        .connect_mux(destination)
        .await
        .with_context(|| format!("opening SSH mux to {destination}"))
}

/// Stream `SCRIPT` over stdin to `sh -s` on the remote host and collect
/// stdout. Any non-zero exit, stderr noise, or timeout is an error.
pub async fn run_script(session: &openssh::Session, run_timeout: Duration) -> Result<String> {
    let fut = async {
        let mut child = session
            .command("sh")
            .arg("-s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .await
            .context("spawning remote sh")?;

        // Feed the embedded script into the remote shell's stdin, then close.
        // `take()` moves the ChildStdin out of the Option so we own it and
        // it gets dropped (closing the pipe) when this scope ends.
        let mut stdin = child.stdin().take().context("remote stdin unavailable")?;
        stdin
            .write_all(SCRIPT.as_bytes())
            .await
            .context("writing script to remote stdin")?;
        stdin.flush().await.ok();
        drop(stdin);

        let output = child.wait_with_output().await.context("waiting for remote sh")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "remote script exited {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        let stdout = String::from_utf8(output.stdout).context("remote stdout not UTF-8")?;
        Ok::<_, anyhow::Error>(stdout)
    };

    match timeout(run_timeout, fut).await {
        Ok(inner) => inner,
        Err(_) => Err(anyhow!("remote script timed out after {:?}", run_timeout)),
    }
}
