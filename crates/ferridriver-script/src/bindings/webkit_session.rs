//! `WebKitSessionJs`: QuickJS binding for raw `WebKit` Inspector Protocol
//! access.
//!
//! The `WebKit` counterpart of `CDPSession`, mirroring the core
//! [`ferridriver::webkit_session::WebKitSession`]: `send(method, params?)`
//! reaches the page's live main target, and `events()` streams that
//! target's events in wire order. Created via
//! `browserContext.newWebKitSession(page)`; `WebKit`-only.

use std::sync::Arc;

use rquickjs::function::Opt;
use rquickjs::{Ctx, JsLifetime, Value, class::Trace};

use crate::bindings::convert::{FerriResultCtxExt, json_to_js, serde_from_js};

#[derive(JsLifetime, Trace)]
#[rquickjs::class(rename = "WebKitSession")]
pub struct WebKitSessionJs {
  #[qjs(skip_trace)]
  inner: ferridriver::webkit_session::WebKitSession,
}

impl WebKitSessionJs {
  #[must_use]
  pub fn new(inner: ferridriver::webkit_session::WebKitSession) -> Self {
    Self { inner }
  }
}

#[rquickjs::methods]
impl WebKitSessionJs {
  /// Send a raw protocol command to the target that is live at the call.
  #[qjs(rename = "send")]
  pub async fn send<'js>(
    &self,
    ctx: Ctx<'js>,
    method: String,
    params: Opt<Value<'js>>,
  ) -> rquickjs::Result<Value<'js>> {
    let params_json = match params.0 {
      Some(v) if !v.is_undefined() && !v.is_null() => serde_from_js(&ctx, v)?,
      _ => serde_json::Value::Object(serde_json::Map::new()),
    };
    let result = self.inner.send(&method, params_json).await.into_js_with(&ctx)?;
    json_to_js(&ctx, &result)
  }

  /// The id of the page's live target. It changes on a cross-process
  /// navigation.
  #[qjs(rename = "targetId")]
  pub fn target_id(&self) -> String {
    self.inner.target_id()
  }

  /// The live target's events from now on, in wire order.
  #[qjs(rename = "events")]
  pub fn events<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<Value<'js>> {
    let events = WebKitEventsJs {
      inner: Arc::new(tokio::sync::Mutex::new(self.inner.events())),
    };
    let instance = rquickjs::class::Class::instance(ctx.clone(), events)?;
    rquickjs::IntoJs::into_js(instance, &ctx)
  }
}

#[derive(JsLifetime, Trace)]
#[rquickjs::class(rename = "WebKitEvents")]
pub struct WebKitEventsJs {
  #[qjs(skip_trace)]
  inner: Arc<tokio::sync::Mutex<ferridriver::webkit_session::WebKitEvents>>,
}

#[rquickjs::methods]
impl WebKitEventsJs {
  /// The next event as `{ method, params }`, or `null` once the target has
  /// gone away: the page closed, or a cross-process navigation replaced it.
  #[qjs(rename = "next")]
  pub async fn next<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<Value<'js>> {
    let inner = Arc::clone(&self.inner);
    let event = inner.lock().await.next().await;
    match event {
      Some(event) => json_to_js(
        &ctx,
        &serde_json::json!({ "method": event.method, "params": event.params }),
      ),
      None => Ok(Value::new_null(ctx)),
    }
  }
}
