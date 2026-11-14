use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context as _, ensure};
use ferridriver::BrowserType;
use ferridriver::options::LaunchOptions;

const HUNG: Duration = Duration::from_secs(120);

const SHORT_MS: u64 = 500;

const SILENT_BROWSER: &str = "#!/bin/sh\nexec sleep 120\n";

static SILENT: OnceLock<Result<(tempfile::TempDir, PathBuf), String>> = OnceLock::new();

fn written() -> anyhow::Result<(tempfile::TempDir, PathBuf)> {
  let folder = tempfile::tempdir()?;
  let path = folder.path().join("silent-browser");
  std::fs::write(&path, SILENT_BROWSER)?;
  std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
  Ok((folder, path))
}

fn silent_browser() -> anyhow::Result<&'static PathBuf> {
  SILENT
    .get_or_init(|| written().map_err(|error| error.to_string()))
    .as_ref()
    .map(|(_, path)| path)
    .map_err(|error| anyhow::anyhow!("the silent browser could not be written: {error}"))
}

async fn a_short_launch_timeout_stops_a_browser_that_never_answers_its_handshake(
  browser: BrowserType,
) -> anyhow::Result<()> {
  let outcome = tokio::time::timeout(
    HUNG,
    browser.launch(LaunchOptions {
      headless: Some(true),
      executable_path: Some(silent_browser()?.display().to_string()),
      timeout: Some(SHORT_MS),
      ..Default::default()
    }),
  )
  .await
  .context("the launch hung past the hung-test limit")?;
  let error = outcome
    .err()
    .context("a browser that never answered its handshake launched")?;
  ensure!(
    error.to_string().contains(&format!("Timeout {SHORT_MS}ms exceeded")),
    "the launch failed otherwise: {error}"
  );
  Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_launch_timeout_stops_a_browser_that_never_answers_its_handshake_on_cdp_pipe() -> anyhow::Result<()> {
  Box::pin(a_short_launch_timeout_stops_a_browser_that_never_answers_its_handshake(
    ferridriver::chromium(),
  ))
  .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_launch_timeout_stops_a_browser_that_never_answers_its_handshake_on_webkit() -> anyhow::Result<()> {
  Box::pin(a_short_launch_timeout_stops_a_browser_that_never_answers_its_handshake(
    ferridriver::webkit(),
  ))
  .await
}
