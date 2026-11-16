//! NAPI bindings for browser installation.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use ferridriver::install::{BrowserInstaller, DependencyGroup};

/// Print the per-method CDP RTT stats table to stderr.
///
/// Only emits when `FERRIDRIVER_RTT_STATS=1` was set at process start
/// (the global aggregator only ticks under that env). Bun and Node
/// process exits do not reliably trigger libc `atexit`, so the CLI
/// bridge calls this just before `process.exit` to make the stats
/// dump appear deterministically.
#[napi]
pub fn dump_rtt_stats() {
  ferridriver::backend::cdp::transport::dump_global_rtt_stats();
}

/// Install the latest stable Chromium browser.
/// Returns the path to the installed chrome executable.
#[napi]
pub async fn install_chromium() -> Result<String> {
  let installer = BrowserInstaller::new();
  installer.install_chromium(|_| {}).await.map_err(crate::error::to_napi)
}

/// Install the system packages `browsers` need (Linux only), like
/// `npx playwright install-deps`: every browser when none are named.
/// Requires root/sudo on Linux. No-op on macOS/Windows.
#[napi(ts_args_type = "browsers?: Array<'chromium' | 'chromium-headless-shell' | 'firefox' | 'webkit'>")]
pub async fn install_system_deps(browsers: Option<Vec<String>>) -> Result<()> {
  let groups = match browsers {
    None => vec![
      DependencyGroup::Chromium,
      DependencyGroup::Firefox,
      DependencyGroup::WebKit,
    ],
    Some(names) => names
      .iter()
      .map(|name| {
        DependencyGroup::of_target(name)
          .ok_or_else(|| Error::new(Status::InvalidArg, format!("no system dependencies for {name:?}")))
      })
      .collect::<Result<_>>()?,
  };
  BrowserInstaller::new()
    .install_system_deps(&groups, |_| {})
    .await
    .map_err(crate::error::to_napi)
}

/// Install Chromium with system dependencies (convenience: install + install-deps).
/// Returns the path to the installed chrome executable.
#[napi]
pub async fn install_chromium_with_deps() -> Result<String> {
  let installer = BrowserInstaller::new();
  installer
    .install_system_deps(&[DependencyGroup::Chromium], |_| {})
    .await
    .map_err(crate::error::to_napi)?;
  installer.install_chromium(|_| {}).await.map_err(crate::error::to_napi)
}

/// Install the latest stable Chrome Headless Shell.
/// Returns the path to the installed chrome-headless-shell executable.
/// This is a lighter, purpose-built binary optimized for headless automation.
#[napi]
pub async fn install_chromium_headless_shell() -> Result<String> {
  let installer = BrowserInstaller::new();
  installer
    .install_chromium_headless_shell(|_| {})
    .await
    .map_err(crate::error::to_napi)
}

/// Find an installed Chromium in the ferridriver cache.
/// Returns the path to the executable or null if not found.
#[napi]
pub fn find_installed_chromium() -> Option<String> {
  BrowserInstaller::new().find_installed_chromium()
}

/// Find an installed Chrome Headless Shell in the ferridriver cache.
/// Returns the path to the executable or null if not found.
#[napi]
pub fn find_installed_headless_shell() -> Option<String> {
  BrowserInstaller::new().find_installed_headless_shell()
}

/// Install the latest stable Firefox.
/// Returns the path to the installed firefox executable.
#[napi]
pub async fn install_firefox() -> Result<String> {
  let installer = BrowserInstaller::new();
  installer.install_firefox(|_| {}).await.map_err(crate::error::to_napi)
}

/// Find an installed Firefox in the ferridriver cache.
/// Returns the path to the executable or null if not found.
#[napi]
pub fn find_installed_firefox() -> Option<String> {
  BrowserInstaller::new().find_installed_firefox()
}

/// Where `installWebkitRevision` installs, and which hosts it downloads from.
#[napi(object)]
#[derive(Default)]
pub struct InstallWebkitRevisionOptions {
  /// The browser cache to install into. Defaults to Playwright's own:
  /// `PLAYWRIGHT_BROWSERS_PATH` when it names a directory, otherwise
  /// `ms-playwright` under the platform cache directory.
  pub cache_dir: Option<String>,
  /// Hosts to download from, tried in order, instead of Playwright's CDN
  /// mirrors, as `PLAYWRIGHT_DOWNLOAD_HOST` does for Playwright. Each serves
  /// `builds/webkit/<revision>/<archive>`.
  pub download_hosts: Option<Vec<String>>,
}

/// Install WebKit build `revision` into Playwright's browser cache, as
/// `playwright install webkit` lays it out, for a project pinned to the
/// `playwright-core` whose `browsers.json` names that revision. A build
/// already marked complete is kept. Returns the path to the build's
/// `pw_run.sh`.
#[napi]
pub async fn install_webkit_revision(
  revision: String,
  options: Option<InstallWebkitRevisionOptions>,
) -> Result<String> {
  let options = options.unwrap_or_default();
  let cache = match options.cache_dir {
    Some(dir) => std::path::PathBuf::from(dir),
    None => ferridriver::install::playwright_cache_dir().ok_or_else(|| {
      Error::new(
        Status::InvalidArg,
        "PLAYWRIGHT_BROWSERS_PATH=0 keeps browsers inside an npm package, so name a cacheDir",
      )
    })?,
  };
  let installer = match options.download_hosts {
    Some(hosts) => BrowserInstaller::new().with_download_hosts(hosts),
    None => BrowserInstaller::new(),
  };
  installer
    .install_webkit_revision(&revision, &cache, |_| {})
    .await
    .map_err(crate::error::to_napi)
}

/// Get the browser cache directory path.
#[napi]
pub fn get_browser_cache_dir() -> String {
  BrowserInstaller::new().cache_dir().to_string_lossy().to_string()
}
