use crate::{EngError, Result};
use ort::session::Session;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

struct PoolInner {
    available: std::sync::Mutex<Vec<Session>>,
    semaphore: Arc<Semaphore>,
}

/// Pool of ONNX inference sessions for parallel reranking.
///
/// Sessions are checked out exclusively via `acquire()` and returned
/// automatically when the `PooledSession` is dropped. The semaphore
/// ensures callers block until a session is available.
pub struct SessionPool {
    inner: Arc<PoolInner>,
    size: usize,
}

/// A session checked out from the pool with exclusive ownership.
/// On drop, the session is returned to the pool and the semaphore
/// permit is released.
pub struct PooledSession {
    session: Option<Session>,
    inner: Arc<PoolInner>,
    _permit: OwnedSemaphorePermit,
}

// SAFETY: ort::Session contains raw pointers and is !Send/!Sync, but it is
// safe to transfer between threads when access is exclusive. SessionPool
// guards availability via semaphore + Vec checkout (only one owner at a time).
// PooledSession holds exclusive ownership of its Session.
unsafe impl Send for SessionPool {}
unsafe impl Sync for SessionPool {}
unsafe impl Send for PooledSession {}
unsafe impl Sync for PooledSession {}

impl SessionPool {
    pub fn new(sessions: Vec<Session>) -> Self {
        let count = sessions.len();
        Self {
            inner: Arc::new(PoolInner {
                available: std::sync::Mutex::new(sessions),
                semaphore: Arc::new(Semaphore::new(count)),
            }),
            size: count,
        }
    }

    /// Acquire exclusive ownership of a session from the pool.
    /// Blocks (async) until a session is available.
    pub async fn acquire(&self) -> Result<PooledSession> {
        let permit = self
            .inner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| EngError::Internal(format!("session pool semaphore closed: {}", e)))?;

        let session = {
            let mut available = self
                .inner
                .available
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            available
                .pop()
                .expect("semaphore guarantees a session is available")
        };

        Ok(PooledSession {
            session: Some(session),
            inner: self.inner.clone(),
            _permit: permit,
        })
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

impl PooledSession {
    /// Get exclusive mutable access to the underlying ONNX session.
    pub fn session_mut(&mut self) -> &mut Session {
        self.session
            .as_mut()
            .expect("session already returned to pool")
    }
}

impl Drop for PooledSession {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            let mut available = self
                .inner
                .available
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            available.push(session);
            // OwnedSemaphorePermit releases on drop after this
        }
    }
}
