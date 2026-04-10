use crate::{EngError, Result};
use ort::session::Session;
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Pool of ONNX inference sessions for parallel reranking.
///
/// Uses a semaphore to limit concurrency to the number of sessions,
/// with try_lock probing to find an available session quickly.
pub struct SessionPool {
    sessions: Vec<Arc<Mutex<Session>>>,
    semaphore: Arc<Semaphore>,
}

/// A session checked out from the pool. The semaphore permit is released on drop,
/// making the session available for the next caller.
pub struct PooledSession {
    session: Arc<Mutex<Session>>,
    _permit: OwnedSemaphorePermit,
}

// SAFETY: Sessions are always accessed through std::sync::Mutex, ensuring exclusive access.
// The ort Session is safe to transfer between threads when protected by a Mutex.
unsafe impl Send for SessionPool {}
unsafe impl Sync for SessionPool {}
unsafe impl Send for PooledSession {}
unsafe impl Sync for PooledSession {}

impl SessionPool {
    pub fn new(sessions: Vec<Session>) -> Self {
        let count = sessions.len();
        Self {
            sessions: sessions
                .into_iter()
                .map(|s| Arc::new(Mutex::new(s)))
                .collect(),
            semaphore: Arc::new(Semaphore::new(count)),
        }
    }

    /// Acquire a session from the pool. Blocks (async) until a session is available.
    pub async fn acquire(&self) -> Result<PooledSession> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| EngError::Internal(format!("session pool semaphore closed: {}", e)))?;

        // Probe for an unlocked session
        for session in &self.sessions {
            if session.try_lock().is_ok() {
                return Ok(PooledSession {
                    session: session.clone(),
                    _permit: permit,
                });
            }
        }

        // Semaphore guarantees at least one will free; fall back to first
        Ok(PooledSession {
            session: self.sessions[0].clone(),
            _permit: permit,
        })
    }

    pub fn size(&self) -> usize {
        self.sessions.len()
    }
}

impl PooledSession {
    /// Lock the underlying session for inference. Uses std::sync::Mutex
    /// so this works in synchronous contexts (spawn_blocking).
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
