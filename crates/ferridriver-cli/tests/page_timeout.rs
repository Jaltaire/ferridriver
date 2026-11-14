use std::io::{Read as _, Write as _};
use std::time::Duration;

use anyhow::{Context as _, ensure};
use ferridriver::options::LaunchOptions;
use ferridriver::protocol::{SerializedArgument, SerializedValue};
use ferridriver::{Browser, BrowserType, Page};

const HUNG: Duration = Duration::from_secs(120);

const SHORT_MS: u64 = 500;

const HELD_BODY: &str = "held";

const FETCH_HELD: &str = "async () => (await fetch('/held')).text()";

struct HeldPage {
  origin: String,
  asked: tokio::sync::mpsc::UnboundedReceiver<()>,
  release: std::sync::mpsc::Sender<()>,
}

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
          ("text/plain", String::from(HELD_BODY))
        } else {
          (
            "text/html",
            String::from("<!doctype html><title>Held</title><p>held</p>"),
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

async fn opened(browser: &Browser, held: &HeldPage) -> anyhow::Result<std::sync::Arc<Page>> {
  let page = browser.new_page().await?;
  page.set_default_timeout(0);
  page.set_default_navigation_timeout(u64::try_from(HUNG.as_millis())?);
  page.goto(&format!("{}/", held.origin)).await?;
  Ok(page)
}

async fn asked(held: &mut HeldPage, what: &str) -> anyhow::Result<()> {
  tokio::time::timeout(HUNG, held.asked.recv())
    .await
    .with_context(|| format!("the page never asked for {what}"))?
    .context("the server stopped")
}

async fn a_zero_timeout_evaluate_waits_for_the_page(browser: Browser) -> anyhow::Result<()> {
  let mut held = serve_held()?;
  let page = opened(&browser, &held).await?;
  let evaluating = {
    let page = std::sync::Arc::clone(&page);
    tokio::spawn(async move {
      page
        .evaluate(FETCH_HELD, SerializedArgument::default(), Some(true))
        .await
    })
  };
  asked(&mut held, "the held answer").await?;
  ensure!(
    !evaluating.is_finished(),
    "an evaluate with no timeout returned before the page answered: {:?}",
    evaluating.await
  );
  held.release.send(()).context("the server stopped")?;
  let answer = tokio::time::timeout(HUNG, evaluating).await???;
  ensure!(
    answer == SerializedValue::Str(HELD_BODY.to_string()),
    "the evaluate returned {answer:?}"
  );
  browser.close().await?;
  Ok(())
}

async fn a_short_timeout_stops_an_evaluate_the_page_never_answers(browser: Browser) -> anyhow::Result<()> {
  let held = serve_held()?;
  let page = opened(&browser, &held).await?;
  page.set_default_timeout(SHORT_MS);
  let outcome = tokio::time::timeout(
    HUNG,
    page.evaluate(FETCH_HELD, SerializedArgument::default(), Some(true)),
  )
  .await?;
  let error = outcome.err().context("an evaluate held past its timeout succeeded")?;
  ensure!(
    error.to_string().contains(&format!("Timeout {SHORT_MS}ms exceeded")),
    "the evaluate failed otherwise: {error}"
  );
  drop(held.release);
  browser.close().await?;
  Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_zero_timeout_evaluate_waits_for_the_page_on_cdp_pipe() -> anyhow::Result<()> {
  Box::pin(a_zero_timeout_evaluate_waits_for_the_page(
    launch(ferridriver::chromium()).await?,
  ))
  .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_zero_timeout_evaluate_waits_for_the_page_on_webkit() -> anyhow::Result<()> {
  Box::pin(a_zero_timeout_evaluate_waits_for_the_page(
    launch(ferridriver::webkit()).await?,
  ))
  .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_timeout_stops_an_evaluate_the_page_never_answers_on_cdp_pipe() -> anyhow::Result<()> {
  Box::pin(a_short_timeout_stops_an_evaluate_the_page_never_answers(
    launch(ferridriver::chromium()).await?,
  ))
  .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_timeout_stops_an_evaluate_the_page_never_answers_on_webkit() -> anyhow::Result<()> {
  Box::pin(a_short_timeout_stops_an_evaluate_the_page_never_answers(
    launch(ferridriver::webkit()).await?,
  ))
  .await
}
