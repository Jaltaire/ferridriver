use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use crate::{FerriError, Result};

tokio::task_local! {
  static CURRENT: Option<OperationBudget>;
  static FALLBACKS: std::sync::Arc<std::sync::atomic::AtomicU64>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct OperationBudget {
  deadline: crate::pause::Deadline,
  timeout_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Expired {
  pub(crate) timeout_ms: u64,
}

impl Expired {
  pub(crate) fn error(self, operation: impl Into<String>) -> FerriError {
    FerriError::timeout(operation, self.timeout_ms)
  }
}

impl OperationBudget {
  pub(crate) fn new(timeout_ms: u64) -> Result<Self> {
    Self::create(timeout_ms, Some(crate::pause::pause_clock()))
  }

  pub(crate) fn wall_clock(timeout_ms: u64) -> Result<Self> {
    Self::create(timeout_ms, None)
  }

  fn create(timeout_ms: u64, clock: Option<&'static crate::pause::PauseClock>) -> Result<Self> {
    let deadline = if timeout_ms == 0 {
      None
    } else {
      Some(
        Instant::now()
          .checked_add(Duration::from_millis(timeout_ms))
          .ok_or_else(|| FerriError::invalid_argument("timeout", "operation timeout is too large"))?,
      )
    };
    Ok(Self {
      deadline: crate::pause::Deadline::new(
        clock,
        deadline,
        clock.map_or(Duration::ZERO, crate::pause::PauseClock::parked_now),
      ),
      timeout_ms,
    })
  }

  pub(crate) fn operation(timeout_ms: u64) -> Result<Self> {
    let requested = Self::new(timeout_ms)?;
    Ok(match (Self::capture(), requested.effective_deadline()) {
      (Some(parent), Some(deadline)) if parent.effective_deadline().is_some_and(|limit| limit <= deadline) => parent,
      (Some(parent), None) => parent,
      _ => requested,
    })
  }

  pub(crate) fn capture() -> Option<Self> {
    CURRENT.try_with(|budget| *budget).ok().flatten()
  }

  pub(crate) fn current(default_ms: u64) -> Result<Self> {
    if let Some(budget) = Self::capture() {
      Ok(budget)
    } else {
      let _ = FALLBACKS.try_with(|count| count.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
      Self::new(default_ms)
    }
  }

  pub(crate) fn scope<F: Future>(self, future: F) -> impl Future<Output = F::Output> {
    CURRENT.scope(Some(self), future)
  }

  pub(crate) fn timeout_ms(self) -> u64 {
    self.timeout_ms
  }

  fn effective_deadline(self) -> Option<Instant> {
    self.deadline.effective()
  }

  pub(crate) fn remaining_ms(self) -> std::result::Result<Option<u64>, Expired> {
    self
      .effective_deadline()
      .map(|deadline| {
        let remaining = deadline
          .checked_duration_since(Instant::now())
          .filter(|remaining| !remaining.is_zero())
          .ok_or(Expired {
            timeout_ms: self.timeout_ms,
          })?;
        Ok(u64::try_from(remaining.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX))
      })
      .transpose()
  }

  pub(crate) fn wait<F: Future>(&self, future: F) -> impl Future<Output = std::result::Result<F::Output, Expired>> {
    use futures::TryFutureExt as _;
    let timeout_ms = self.timeout_ms;
    crate::pause::with_deadline(&self.deadline, future).map_err(move |_| Expired { timeout_ms })
  }

  pub(crate) async fn send<T>(&self, sender: &tokio::sync::mpsc::Sender<T>, value: T) -> Result<()> {
    use tokio::sync::mpsc::error::TrySendError;
    self
      .remaining_ms()
      .map_err(|error| error.error("queueing protocol command"))?;
    let permit = match sender.try_reserve() {
      Ok(permit) => permit,
      Err(TrySendError::Closed(())) => return Err(FerriError::target_closed(Some("Protocol writer closed".into()))),
      Err(TrySendError::Full(())) => {
        // Keep the semaphore waiter off every caller's stack; only saturated queues allocate it.
        Box::pin(self.wait(sender.reserve()))
          .await
          .map_err(|error| error.error("queueing protocol command"))?
          .map_err(|_| FerriError::target_closed(Some("Protocol writer closed".into())))?
      },
    };
    self
      .remaining_ms()
      .map_err(|error| error.error("queueing protocol command"))?;
    permit.send(value);
    Ok(())
  }
}

/// Milliseconds the current operation has left: `None` outside one or
/// for one without a timeout, `Some(0)` once its deadline has passed.
pub(crate) fn operation_remaining_ms() -> Option<u64> {
  OperationBudget::capture().and_then(|budget| match budget.remaining_ms() {
    Ok(left) => left,
    Err(_) => Some(0),
  })
}

/// Run `future` as one public operation with `timeout_ms` (`0`: none), the
/// way Playwright's `ProgressController` races a call and every protocol
/// command it sends against one deadline: each command waits until that
/// deadline rather than the transport's own default, and the operation
/// ends there whatever it is waiting on. Nested inside another operation,
/// the tighter deadline wins. An operation without a timeout arms no timer.
///
/// A command cut off by the deadline fails as the command it was, so once
/// the deadline has passed a timeout is reported as `operation` timing out,
/// unless it already carries the operation's own `Call log:`.
pub(crate) async fn scoped<T>(
  timeout_ms: u64,
  operation: impl FnOnce() -> String,
  future: impl Future<Output = Result<T>>,
) -> Result<T> {
  let budget = OperationBudget::operation(timeout_ms)?;
  match budget.scope(budget.wait(future)).await {
    Err(Expired { timeout_ms }) => Err(FerriError::timeout(operation(), timeout_ms)),
    Ok(Err(FerriError::Timeout { call_log, .. })) if call_log.is_empty() && budget.remaining_ms().is_err() => {
      Err(FerriError::timeout(operation(), budget.timeout_ms()))
    },
    Ok(result) => result,
  }
}

pub(crate) fn scoped_boxed<'a, T: Send + 'a>(
  timeout_ms: u64,
  operation: impl FnOnce() -> String + Send + 'a,
  future: impl Future<Output = Result<T>> + Send + 'a,
) -> futures::future::BoxFuture<'a, Result<T>> {
  futures::FutureExt::boxed(scoped(timeout_ms, operation, future))
}

pub(crate) type TimeoutSource = std::sync::Arc<dyn Fn() -> Option<u64> + Send + Sync>;

#[derive(Default)]
pub(crate) struct OwnerTimeouts {
  sources: std::sync::RwLock<rustc_hash::FxHashMap<std::sync::Arc<str>, TimeoutSource>>,
}

impl OwnerTimeouts {
  pub(crate) fn register(&self, owner: std::sync::Arc<str>, source: TimeoutSource) {
    let existing: Vec<(std::sync::Arc<str>, TimeoutSource)> = self
      .sources
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .iter()
      .map(|(owner, source)| (std::sync::Arc::clone(owner), std::sync::Arc::clone(source)))
      .collect();
    let gone: Vec<std::sync::Arc<str>> = existing
      .into_iter()
      .filter(|(_, source)| source().is_none())
      .map(|(owner, _)| owner)
      .collect();
    let mut sources = self.sources.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    for owner in &gone {
      sources.remove(owner);
    }
    sources.insert(owner, source);
  }

  pub(crate) fn inherit(&self, from: &str, to: &str) {
    let mut sources = self.sources.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(source) = sources.get(from).cloned() {
      sources.insert(std::sync::Arc::from(to), source);
    }
  }

  pub(crate) fn forget(&self, owner: &str) {
    self
      .sources
      .write()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .remove(owner);
  }

  fn owned_ms(&self, owner: &str) -> Option<u64> {
    let source = self
      .sources
      .read()
      .unwrap_or_else(std::sync::PoisonError::into_inner)
      .get(owner)
      .cloned()?;
    source()
  }

  pub(crate) fn budget(&self, owner: Option<&str>, default_ms: u64) -> Result<OperationBudget> {
    if let Some(budget) = OperationBudget::capture() {
      return Ok(budget);
    }
    match owner.and_then(|owner| self.owned_ms(owner)) {
      Some(timeout_ms) => OperationBudget::new(timeout_ms),
      None => OperationBudget::current(default_ms),
    }
  }
}

pub(crate) fn enclosing_or(default_ms: u64) -> u64 {
  OperationBudget::capture().map_or(default_ms, |_| 0)
}

pub(crate) fn within_boxed<'a, T: Send + 'a>(
  timeout_ms: u64,
  future: impl Future<Output = Result<T>> + Send + 'a,
) -> futures::future::BoxFuture<'a, Result<T>> {
  futures::FutureExt::boxed(async move { OperationBudget::operation(timeout_ms)?.scope(future).await })
}

#[doc(hidden)]
pub async fn fallbacks_during<F: Future>(future: F) -> (F::Output, u64) {
  let count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
  let output = FALLBACKS.scope(std::sync::Arc::clone(&count), future).await;
  (output, count.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn answering(timeout_ms: Option<u64>) -> TimeoutSource {
    std::sync::Arc::new(move || timeout_ms)
  }

  #[tokio::test(start_paused = true)]
  async fn an_owner_bounds_its_commands_by_its_own_timeout_and_a_scope_still_wins() {
    let owners = OwnerTimeouts::default();
    owners.register(std::sync::Arc::from("page"), answering(Some(60_000)));
    let (budget, fallbacks) = fallbacks_during(async { owners.budget(Some("page"), 30_000).unwrap() }).await;
    assert_eq!((budget.timeout_ms(), fallbacks), (60_000, 0));
    let (budget, fallbacks) = fallbacks_during(async { owners.budget(Some("stranger"), 30_000).unwrap() }).await;
    assert_eq!((budget.timeout_ms(), fallbacks), (30_000, 1));
    let (budget, fallbacks) = fallbacks_during(async { owners.budget(None, 30_000).unwrap() }).await;
    assert_eq!((budget.timeout_ms(), fallbacks), (30_000, 1));
    let scoped = OperationBudget::new(500)
      .unwrap()
      .scope(async { owners.budget(Some("page"), 30_000).unwrap() })
      .await;
    assert_eq!(scoped.timeout_ms(), 500);
  }

  #[tokio::test(start_paused = true)]
  async fn an_owner_whose_page_is_gone_falls_back_and_is_swept_by_the_next_registration() {
    let owners = OwnerTimeouts::default();
    let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let answer = std::sync::Arc::clone(&alive);
    owners.register(
      std::sync::Arc::from("closed"),
      std::sync::Arc::new(move || answer.load(std::sync::atomic::Ordering::Relaxed).then_some(0_u64)),
    );
    assert_eq!(
      owners.budget(Some("closed"), 30_000).unwrap().remaining_ms().unwrap(),
      None
    );
    alive.store(false, std::sync::atomic::Ordering::Relaxed);
    let (budget, fallbacks) = fallbacks_during(async { owners.budget(Some("closed"), 30_000).unwrap() }).await;
    assert_eq!((budget.timeout_ms(), fallbacks), (30_000, 1));
    owners.register(std::sync::Arc::from("open"), answering(Some(1_000)));
    assert!(!owners.sources.read().unwrap().contains_key("closed"));
    owners.forget("open");
    assert!(owners.sources.read().unwrap().is_empty());
  }

  #[tokio::test(start_paused = true)]
  async fn a_popup_inherits_its_openers_timeout_until_it_registers_its_own() {
    let owners = OwnerTimeouts::default();
    owners.inherit("opener", "orphan");
    assert!(owners.sources.read().unwrap().is_empty());
    owners.register(std::sync::Arc::from("opener"), answering(Some(45_000)));
    owners.inherit("opener", "popup");
    assert_eq!(owners.budget(Some("popup"), 30_000).unwrap().timeout_ms(), 45_000);
    owners.register(std::sync::Arc::from("popup"), answering(Some(5_000)));
    assert_eq!(owners.budget(Some("popup"), 30_000).unwrap().timeout_ms(), 5_000);
    assert_eq!(owners.budget(Some("opener"), 30_000).unwrap().timeout_ms(), 45_000);
  }

  #[tokio::test(start_paused = true)]
  async fn a_command_of_an_owner_with_a_longer_timeout_outlasts_the_old_fixed_thirty_seconds() {
    let owners = OwnerTimeouts::default();
    owners.register(std::sync::Arc::from("page"), answering(Some(60_000)));
    let budget = owners.budget(Some("page"), 30_000).unwrap();
    let reply = budget
      .wait(async {
        tokio::time::sleep(Duration::from_secs(45)).await;
        "answered"
      })
      .await;
    assert_eq!(reply.unwrap(), "answered");
  }

  #[tokio::test(start_paused = true)]
  async fn within_boxed_bounds_commands_but_runs_its_future_to_the_end() {
    let started = Instant::now();
    let finished = within_boxed(100, async {
      assert_eq!(OperationBudget::capture().unwrap().timeout_ms(), 100);
      tokio::time::sleep(Duration::from_millis(250)).await;
      assert!(OperationBudget::capture().unwrap().remaining_ms().is_err());
      Ok("cleaned up")
    })
    .await
    .unwrap();
    assert_eq!(finished, "cleaned up");
    assert_eq!(Instant::now() - started, Duration::from_millis(250));
  }

  #[tokio::test(start_paused = true)]
  async fn scoped_boxed_names_the_operation_and_its_callers_timeout() {
    let error = scoped_boxed(500, || "creating a page".into(), async {
      tokio::time::sleep(Duration::from_secs(120)).await;
      Ok(())
    })
    .await
    .unwrap_err();
    assert!(error.is_timeout_error());
    assert!(error.to_string().contains("creating a page"), "{error}");
    assert!(error.to_string().contains("500ms"), "{error}");
    let inner_expiry = scoped_boxed(500, || "closing the page".into(), async {
      Err::<(), _>(
        OperationBudget::current(30_000)?
          .wait(std::future::pending::<()>())
          .await
          .unwrap_err()
          .error("waiting for Target.close reply"),
      )
    })
    .await
    .unwrap_err();
    assert!(inner_expiry.to_string().contains("closing the page"), "{inner_expiry}");
  }

  #[tokio::test(start_paused = true)]
  async fn a_wait_inside_a_scope_follows_it_and_a_wait_outside_follows_its_old_default() {
    assert_eq!(enclosing_or(30_000), 30_000);
    let inside = OperationBudget::new(60_000)
      .unwrap()
      .scope(async {
        (
          enclosing_or(30_000),
          OperationBudget::current(2_000).unwrap().timeout_ms(),
        )
      })
      .await;
    assert_eq!(inside, (0, 60_000));
    let (outside, fallbacks) = fallbacks_during(async { OperationBudget::current(2_000).unwrap().timeout_ms() }).await;
    assert_eq!((outside, fallbacks), (2_000, 1));
  }

  #[tokio::test(start_paused = true)]
  async fn queue_capacity_preserves_order_and_releases_cancelled_reservations() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let budget = OperationBudget::new(1000).unwrap();
    budget.send(&sender, 1).await.unwrap();
    let mut cancelled = Box::pin(budget.send(&sender, 2));
    assert!(futures::poll!(&mut cancelled).is_pending());
    drop(cancelled);
    let mut waiting = Box::pin(budget.send(&sender, 3));
    assert!(futures::poll!(&mut waiting).is_pending());
    assert_eq!(receiver.recv().await, Some(1));
    waiting.await.unwrap();
    assert_eq!(receiver.recv().await, Some(3));
    tokio::time::advance(Duration::from_millis(1000)).await;
    assert!(budget.send(&sender, 4).await.unwrap_err().is_timeout_error());
    assert!(receiver.try_recv().is_err());
    receiver.close();
    assert!(matches!(
      OperationBudget::new(0).unwrap().send(&sender, 5).await,
      Err(FerriError::TargetClosed { .. })
    ));
  }

  #[tokio::test]
  async fn debugger_parks_suspend_shared_budgets_without_extending_nested_or_cleanup_limits() {
    crate::pause::with_test_clock(async {
      let budget = OperationBudget::new(30).unwrap();
      let cleanup = OperationBudget::wall_clock(30).unwrap();
      budget
        .wait(async {
          let park = crate::pause::pause_clock().park();
          tokio::time::sleep(Duration::from_millis(80)).await;
          drop(park);
        })
        .await
        .unwrap();
      assert!(cleanup.remaining_ms().is_err());
      budget
        .scope(async {
          assert_eq!(OperationBudget::operation(5).unwrap().timeout_ms(), 5);
        })
        .await;
      assert!(
        budget
          .wait(tokio::time::sleep(Duration::from_millis(80)))
          .await
          .is_err()
      );
    })
    .await;
  }

  #[tokio::test(start_paused = true)]
  async fn unlimited_scope_overrides_transport_defaults_and_nested_operations_cannot_extend_a_parent() {
    OperationBudget::new(0)
      .unwrap()
      .scope(async {
        let command = OperationBudget::current(30_000).unwrap();
        command
          .wait(tokio::time::sleep(Duration::from_secs(120)))
          .await
          .unwrap();
        assert_eq!(command.remaining_ms().unwrap(), None);
      })
      .await;
    OperationBudget::new(1000)
      .unwrap()
      .scope(async {
        tokio::time::sleep(Duration::from_millis(700)).await;
        for timeout in [0, 30_000] {
          let child = OperationBudget::operation(timeout).unwrap();
          assert_eq!(child.remaining_ms().unwrap(), Some(300));
        }
        let child = OperationBudget::operation(100).unwrap();
        assert_eq!(child.remaining_ms().unwrap(), Some(100));
      })
      .await;
    assert!(OperationBudget::capture().is_none());
  }

  #[tokio::test(start_paused = true)]
  async fn a_scoped_operation_owns_the_deadline_of_its_commands() {
    let command = || async {
      OperationBudget::current(30_000)?
        .wait(tokio::time::sleep(Duration::from_secs(45)))
        .await
        .map_err(|error| error.error("waiting for Page.navigate response"))
    };
    scoped(60_000, || "navigating".into(), command()).await.unwrap();
    scoped(0, || "navigating".into(), command()).await.unwrap();
    let started = Instant::now();
    let error = scoped(1000, || "navigating".into(), command()).await.unwrap_err();
    assert_eq!(error.to_string(), "Timeout 1000ms exceeded while navigating");
    assert_eq!(Instant::now() - started, Duration::from_millis(1000));
    let started = Instant::now();
    assert!(command().await.unwrap_err().is_timeout_error());
    assert_eq!(Instant::now() - started, Duration::from_secs(30));
  }

  #[tokio::test(start_paused = true)]
  async fn sequential_replies_share_one_absolute_deadline() {
    let budget = OperationBudget::new(1000).unwrap();
    budget
      .wait(tokio::time::sleep(Duration::from_millis(700)))
      .await
      .unwrap();
    let started = Instant::now();
    let error = budget
      .wait(tokio::time::sleep(Duration::from_millis(700)))
      .await
      .unwrap_err();
    assert_eq!(error.timeout_ms, 1000);
    assert_eq!(Instant::now() - started, Duration::from_millis(300));
    let mut invoked = false;
    assert!(budget.wait(async { invoked = true }).await.is_err());
    assert!(!invoked);
  }
}
