//! `WebKitSession` — NAPI binding for raw `WebKit` Inspector Protocol access.
//!
//! The `WebKit` counterpart of `CDPSession`, mirroring the core
//! [`ferridriver::webkit_session::WebKitSession`]: `send(method, params?)`
//! reaches the page's live main target, and `events()` streams that
//! target's events in wire order. Created via
//! `browserContext.newWebKitSession(page)`; `WebKit`-only.

use napi::Result;
use napi_derive::napi;

use crate::error::IntoNapi;

/// Raw `WebKit` Inspector Protocol session on a page's main target.
#[napi]
pub struct WebKitSession {
  inner: ferridriver::webkit_session::WebKitSession,
}

impl WebKitSession {
  pub(crate) fn wrap(inner: ferridriver::webkit_session::WebKitSession) -> Self {
    Self { inner }
  }
}

#[napi]
impl WebKitSession {
  /// Send a raw protocol command to the target that is live at the call,
  /// and resolve with its result. Rejects once the page has closed.
  #[napi(ts_args_type = "method: string, params?: object", ts_return_type = "Promise<any>")]
  pub async fn send(&self, method: String, params: Option<serde_json::Value>) -> Result<serde_json::Value> {
    self
      .inner
      .send(
        &method,
        params.unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
      )
      .await
      .into_napi()
  }

  /// The id of the page's live target. It changes on a cross-process
  /// navigation.
  #[napi]
  pub fn target_id(&self) -> String {
    self.inner.target_id()
  }

  /// The live target's events from now on, in wire order. The stream ends
  /// when that target goes away; take another after a cross-process
  /// navigation to follow the new one.
  #[napi]
  pub fn events(&self) -> WebKitEvents {
    WebKitEvents {
      inner: tokio::sync::Mutex::new(self.inner.events()),
    }
  }
}

/// One protocol event from a page's target.
#[napi(object)]
pub struct WebKitEvent {
  /// The event's method, such as `"Console.messageAdded"`.
  pub method: String,
  /// The event's `params` object.
  pub params: serde_json::Value,
}

/// The events of one target, in wire order.
#[napi]
pub struct WebKitEvents {
  inner: tokio::sync::Mutex<ferridriver::webkit_session::WebKitEvents>,
}

#[napi]
impl WebKitEvents {
  /// The next event, or `null` once the target has gone away: the page
  /// closed, or a cross-process navigation replaced the target.
  #[napi(ts_return_type = "Promise<WebKitEvent | null>")]
  pub async fn next(&self) -> Option<WebKitEvent> {
    self.inner.lock().await.next().await.map(|event| WebKitEvent {
      method: event.method,
      params: event.params,
    })
  }
}
