//! A navigation timeout of `0` means no timeout, as Playwright's
//! `timeout: 0` does: the navigation waits for its load however long the
//! page takes. A short timeout still stops a navigation that does not load.

use std::io::{Read as _, Write as _};
use std::time::Duration;

use anyhow::{Context as _, ensure};
use ferridriver::options::LaunchOptions;
use ferridriver::{Browser, BrowserType};

/// Only stops a hung test; the work takes a fraction of it.
const HUNG: Duration = Duration::from_secs(120);

/// A page whose load waits on `/held`, and the means to see `/held` asked
/// for and to answer it.
struct HeldPage {
  origin: String,
  asked: tokio::sync::mpsc::UnboundedReceiver<()>,
  release: std::sync::mpsc::Sender<()>,
}

/// Serves `/` at once, holds `/held` until the test releases it, a thread
/// per connection and every reply closing it.
fn serve_held() -> anyhow::Result<HeldPage> {
  let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
  let origin = format!("http://{}", listener.local_addr()?);
  let (asked_tx, asked) = tokio::sync::mpsc::unbounded_channel();
  let (release, released) = std::sync::mpsc::channel::<()>();
  let released = std::sync::Arc::new(std::sync::Mutex::new(released));
  std::thread::spawn(move || {
    for stream in listener.incoming().flatten() {
      let asked_tx = asked_tx.clone();
      let released = std::sync::Arc::clone(&released);
      std::thread::spawn(move || {
        let mut stream = stream;
        let mut request = [0_u8; 4096];
        let read = stream.read(&mut request).unwrap_or(0);
        let head = String::from_utf8_lossy(&request[..read]);
        let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
        let (kind, body) = if path == "/held" {
          let _ = asked_tx.send(());
          let _ = released.lock().map(|released| released.recv());
          ("text/css", String::from("p { color: green }"))
        } else {
          (
            "text/html",
            String::from("<!doctype html><title>Held</title><link rel=stylesheet href=/held><p>held</p>"),
          )
        };
        let _ = write!(
          stream,
          "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
          body.len()
        );
      });
    }
  });
  Ok(HeldPage { origin, asked, release })
}

async fn launch(browser: BrowserType) -> anyhow::Result<Browser> {
  Ok(
    browser
      .launch(LaunchOptions {
        headless: Some(true),
        ..Default::default()
      })
      .await?,
  )
}

async fn a_zero_timeout_waits_for_the_load(browser: Browser) -> anyhow::Result<()> {
  let mut held = serve_held()?;
  let page = browser.new_page().await?;
  page.set_default_navigation_timeout(0);
  let navigating = {
    let page = std::sync::Arc::clone(&page);
    let url = format!("{}/", held.origin);
    tokio::spawn(async move { page.goto(&url).await.map(|_| ()) })
  };
  tokio::time::timeout(HUNG, held.asked.recv())
    .await
    .context("the page never asked for its held stylesheet")?
    .context("the server stopped")?;
  ensure!(
    !navigating.is_finished(),
    "a navigation with no timeout returned before its load: {:?}",
    navigating.await
  );
  held.release.send(()).context("the server stopped")?;
  tokio::time::timeout(HUNG, navigating).await???;
  let state: String = page.eval("() => document.readyState").await?;
  ensure!(state == "complete", "the navigation returned at readyState {state}");
  browser.close().await?;
  Ok(())
}

async fn a_short_timeout_stops_a_navigation_that_does_not_load(browser: Browser) -> anyhow::Result<()> {
  let held = serve_held()?;
  let page = browser.new_page().await?;
  page.set_default_navigation_timeout(500);
  let outcome = tokio::time::timeout(HUNG, page.goto(&format!("{}/", held.origin))).await?;
  let error = outcome.err().context("a navigation held past its timeout succeeded")?;
  ensure!(
    error.to_string().contains("Timeout 500ms exceeded"),
    "the navigation failed otherwise: {error}"
  );
  drop(held.release);
  browser.close().await?;
  Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_zero_timeout_waits_for_the_load_on_cdp_pipe() -> anyhow::Result<()> {
  Box::pin(a_zero_timeout_waits_for_the_load(
    launch(ferridriver::chromium()).await?,
  ))
  .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_zero_timeout_waits_for_the_load_on_webkit() -> anyhow::Result<()> {
  Box::pin(a_zero_timeout_waits_for_the_load(launch(ferridriver::webkit()).await?)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_timeout_stops_a_navigation_that_does_not_load_on_cdp_pipe() -> anyhow::Result<()> {
  Box::pin(a_short_timeout_stops_a_navigation_that_does_not_load(
    launch(ferridriver::chromium()).await?,
  ))
  .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_timeout_stops_a_navigation_that_does_not_load_on_webkit() -> anyhow::Result<()> {
  Box::pin(a_short_timeout_stops_a_navigation_that_does_not_load(
    launch(ferridriver::webkit()).await?,
  ))
  .await
}
