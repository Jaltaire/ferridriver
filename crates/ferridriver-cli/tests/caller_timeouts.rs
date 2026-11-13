use std::future::Future;
use std::io::{Read as _, Write as _};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, ensure};
use ferridriver::options::{InitScriptSource, LaunchOptions};
use ferridriver::protocol::SerializedArgument;
use ferridriver::{Browser, BrowserType, Page};

const HUNG: Duration = Duration::from_secs(120);

const CALLER_MS: u64 = 45_000;

const DOCUMENT: &str = "<!doctype html><title>Caller</title><input id=name><p id=word data-word=caller>caller</p><div id=source draggable=true style='width:40px;height:40px'>drag</div><div id=target style='width:40px;height:40px'>drop</div>";

fn serve() -> anyhow::Result<String> {
  let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
  let origin = format!("http://{}", listener.local_addr()?);
  std::thread::spawn(move || {
    for stream in listener.incoming().flatten() {
      std::thread::spawn(move || {
        let mut stream = stream;
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        let _ = write!(
          stream,
          "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{DOCUMENT}",
          DOCUMENT.len()
        );
      });
    }
  });
  Ok(origin)
}

fn unfallen<'a, T: 'a>(
  what: &'a str,
  operation: impl Future<Output = ferridriver::Result<T>> + 'a,
) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<T>> + 'a>> {
  Box::pin(async move {
    let (outcome, fallbacks) = ferridriver::fallbacks_during(tokio::time::timeout(HUNG, operation)).await;
    let value = outcome
      .with_context(|| format!("{what} hung past the hung-test limit"))?
      .with_context(|| format!("{what} failed"))?;
    ensure!(
      fallbacks == 0,
      "{what} bounded {fallbacks} protocol waits by a fixed budget rather than the caller's timeout"
    );
    Ok(value)
  })
}

async fn launched(browser: BrowserType) -> anyhow::Result<Browser> {
  Ok(
    browser
      .launch(LaunchOptions {
        headless: Some(true),
        timeout: Some(CALLER_MS),
        ..Default::default()
      })
      .await?,
  )
}

async fn opened(browser: &Browser) -> anyhow::Result<(ferridriver::context::ContextRef, Arc<Page>)> {
  let context = browser.new_context().await?;
  context.set_default_timeout(CALLER_MS);
  let page = unfallen("creating a page", context.new_page()).await?;
  page.set_default_timeout(CALLER_MS);
  page.set_default_navigation_timeout(CALLER_MS);
  Ok((context, page))
}

async fn creating_navigating_and_closing_follow_the_callers_timeouts(browser: BrowserType) -> anyhow::Result<()> {
  let origin = serve()?;
  let browser = launched(browser).await?;
  let (context, page) = opened(&browser).await?;
  unfallen("navigating", page.goto(&format!("{origin}/")).into_future()).await?;
  unfallen("reloading", page.reload().into_future()).await?;
  unfallen("navigating again", page.goto(&format!("{origin}/next")).into_future()).await?;
  unfallen("going back", page.go_back().into_future()).await?;
  unfallen("going forward", page.go_forward().into_future()).await?;
  page.set_default_navigation_timeout(0);
  unfallen(
    "navigating with no timeout to a document that requests nothing",
    page.goto("about:blank").into_future(),
  )
  .await?;
  unfallen(
    "reloading with no timeout a document that requests nothing",
    page.reload().into_future(),
  )
  .await?;
  page.set_default_navigation_timeout(CALLER_MS);
  unfallen("setting the content", page.set_content(DOCUMENT).into_future()).await?;
  unfallen("reading the storage state", context.storage_state().into_future()).await?;
  unfallen("closing the page", page.close().into_future()).await?;
  unfallen("closing the context", context.close().into_future()).await?;
  unfallen("closing the browser", browser.close().into_future()).await?;
  Ok(())
}

async fn reading_the_page_follows_the_callers_timeouts(browser: BrowserType) -> anyhow::Result<()> {
  let origin = serve()?;
  let browser = launched(browser).await?;
  let (_context, page) = opened(&browser).await?;
  page.goto(&format!("{origin}/")).await?;
  let word = page.locator("#word");
  let count = unfallen("counting", word.count()).await?;
  ensure!(count == 1, "the locator counted {count}");
  unfallen("measuring", word.bounding_box()).await?;
  let attribute = unfallen("reading an attribute", word.get_attribute("data-word").into_future()).await?;
  ensure!(
    attribute.as_deref() == Some("caller"),
    "the attribute read {attribute:?}"
  );
  unfallen("reading the accessibility tree", word.aria_snapshot().into_future()).await?;
  unfallen(
    "evaluating on an element",
    word
      .evaluate(
        "element => element.textContent",
        SerializedArgument::default(),
        Some(true),
      )
      .into_future(),
  )
  .await?;
  unfallen(
    "evaluating",
    page.evaluate("() => document.title", SerializedArgument::default(), Some(true)),
  )
  .await?;
  unfallen(
    "waiting for a function",
    page.wait_for_function(
      "() => document.readyState === 'complete'",
      SerializedArgument::default(),
      Some(true),
      None,
    ),
  )
  .await?;
  unfallen("taking a screenshot", page.screenshot().into_future()).await?;
  browser.close().await?;
  Ok(())
}

async fn acting_on_the_page_follows_the_callers_timeouts(browser: BrowserType) -> anyhow::Result<()> {
  let origin = serve()?;
  let browser = launched(browser).await?;
  let (_context, page) = opened(&browser).await?;
  page.goto(&format!("{origin}/")).await?;
  unfallen("clicking", page.locator("#name").click().into_future()).await?;
  unfallen("filling", page.locator("#name").fill("caller").into_future()).await?;
  unfallen("hovering", page.locator("#word").hover().into_future()).await?;
  unfallen("pressing a key", page.keyboard().press("Tab").into_future()).await?;
  unfallen("moving the mouse", page.mouse().r#move(5.0, 5.0).into_future()).await?;
  unfallen(
    "dragging",
    page.locator("#source").drag_to(&page.locator("#target")).into_future(),
  )
  .await?;
  browser.close().await?;
  Ok(())
}

async fn configuring_the_page_follows_the_callers_timeouts(browser: BrowserType) -> anyhow::Result<()> {
  let origin = serve()?;
  let browser = launched(browser).await?;
  let (context, page) = opened(&browser).await?;
  page.goto(&format!("{origin}/")).await?;
  unfallen("resizing the viewport", page.set_viewport_size(640, 480)).await?;
  unfallen(
    "adding an init script",
    page.add_init_script(InitScriptSource::Source("window.caller = true;".into()), None),
  )
  .await?;
  unfallen(
    "adding a context init script",
    context.add_init_script(InitScriptSource::Source("window.context = true;".into()), None),
  )
  .await?;
  let matcher = ferridriver::url_matcher::UrlMatcher::glob(format!("{origin}/routed"))?;
  let handler: ferridriver::route::RouteHandler =
    Arc::new(|route| route.continue_route(ferridriver::route::ContinueOverrides::default()));
  unfallen("routing", page.route(matcher, handler, None)).await?;
  let exposed: ferridriver::events::ExposedFn =
    Arc::new(|_| Box::pin(async { Ok(ferridriver::events::BindingReturn::Json(serde_json::json!(true))) }));
  unfallen("exposing a function", page.expose_function("caller", exposed)).await?;
  browser.close().await?;
  Ok(())
}

macro_rules! on_both_engines {
  ($($scenario:ident),* $(,)?) => {
    $(
      mod $scenario {
        #[tokio::test(flavor = "multi_thread")]
        async fn on_cdp_pipe() -> anyhow::Result<()> {
          Box::pin(super::$scenario(ferridriver::chromium())).await
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn on_webkit() -> anyhow::Result<()> {
          Box::pin(super::$scenario(ferridriver::webkit())).await
        }
      }
    )*
  };
}

on_both_engines!(
  creating_navigating_and_closing_follow_the_callers_timeouts,
  reading_the_page_follows_the_callers_timeouts,
  acting_on_the_page_follows_the_callers_timeouts,
  configuring_the_page_follows_the_callers_timeouts,
);
