//! One-shot jobs (doc 01 §3): a `Job<T>` is a future; dropping it cancels the work.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use futures::channel::oneshot;
use serde::{Deserialize, Serialize};

/// Cooperative cancellation flag, checked by the engine at stage/task boundaries (doc 05 §8).
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum JobError {
    #[error("job was cancelled")]
    Cancelled,
    #[error("the engine dropped the job without completing it")]
    Dropped,
    #[error("not supported yet: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Failed(String),
}

pub type JobResult<T> = Result<T, JobError>;

/// Handle to a one-shot job. Await it for the result; drop it to cancel.
#[must_use = "dropping a Job cancels it"]
pub struct Job<T> {
    rx: oneshot::Receiver<JobResult<T>>,
    cancel: CancelToken,
}

/// The engine side of a [`Job`].
pub struct JobCompleter<T> {
    tx: oneshot::Sender<JobResult<T>>,
    cancel: CancelToken,
}

impl<T> Job<T> {
    pub fn new() -> (Job<T>, JobCompleter<T>) {
        let (tx, rx) = oneshot::channel();
        let cancel = CancelToken::new();
        (
            Job {
                rx,
                cancel: cancel.clone(),
            },
            JobCompleter { tx, cancel },
        )
    }

    /// An already-completed job.
    pub fn ready(r: JobResult<T>) -> Job<T> {
        let (job, c) = Job::new();
        c.complete(r);
        job
    }

    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }

    /// Block the current thread until the job finishes (CLI, tests). Never call this from the
    /// GPUI main thread.
    pub fn wait(self) -> JobResult<T> {
        futures::executor::block_on(self)
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl<T> Future for Job<T> {
    type Output = JobResult<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(r)) => Poll::Ready(r),
            Poll::Ready(Err(oneshot::Canceled)) => Poll::Ready(Err(JobError::Dropped)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> JobCompleter<T> {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled() || self.tx.is_canceled()
    }
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }
    pub fn complete(self, r: JobResult<T>) {
        let _ = self.tx.send(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_and_cancel_on_drop() {
        let (job, c) = Job::<u32>::new();
        c.complete(Ok(7));
        assert_eq!(job.wait(), Ok(7));

        let (job, c) = Job::<u32>::new();
        assert!(!c.is_cancelled());
        drop(job);
        assert!(c.is_cancelled());

        let (job, c) = Job::<u32>::new();
        drop(c);
        assert_eq!(job.wait(), Err(JobError::Dropped));
    }
}
