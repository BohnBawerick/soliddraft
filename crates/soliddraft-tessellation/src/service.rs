//! Tessellation off the interface thread.
//!
//! A [`Tessellator`] takes requests (a key, a body and the levels wanted) and answers
//! them on a worker thread with one
//! [`tessellate_levels`](soliddraft_kernel_api::Kernel::tessellate_levels) call each. The
//! interface polls for answers once a frame and never waits on the kernel.
//!
//! Each request for a key gets a new generation. Asking again for the same key, say
//! after a rebuild or a quality change, supersedes the earlier request: the worker skips
//! it if it has not started, and its answer is dropped if it has. [`poll`] only returns
//! answers to the latest request of each key.
//!
//! The caller keeps the body alive until its answer arrives or it cancels the request;
//! a body released early answers with [`KernelError::UnknownBody`].
//!
//! In a WebAssembly build there are no threads, so requests wait in a queue and each
//! [`poll`] answers at most one of them: a frame pays for one kernel call, never for a
//! whole backlog.
//!
//! [`poll`]: Tessellator::poll
//! [`KernelError::UnknownBody`]: soliddraft_kernel_api::KernelError::UnknownBody

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use soliddraft_kernel_api::{BodyId, Kernel, KernelResult, Mesh, TessellationParams};

/// One answered request.
#[derive(Debug)]
pub struct Done<K> {
    /// The request's key.
    pub key: K,
    /// The request's generation, as [`Tessellator::request`] returned it.
    pub generation: u64,
    /// One mesh per level asked for, in order, or why the kernel could not make them.
    pub result: KernelResult<Vec<Mesh>>,
}

struct Job<K> {
    key: K,
    generation: u64,
    body: BodyId,
    levels: Vec<TessellationParams>,
}

/// The latest generation of each key not yet answered or cancelled.
type Latest<K> = Arc<Mutex<HashMap<K, u64>>>;

fn is_latest<K: Eq + Hash>(latest: &Latest<K>, key: &K, generation: u64) -> bool {
    latest.lock().expect("tessellator lock").get(key) == Some(&generation)
}

fn answer<K>(kernel: &dyn Kernel, job: Job<K>) -> Done<K> {
    Done { result: kernel.preview_levels(job.body, &job.levels), key: job.key, generation: job.generation }
}

/// Tessellates bodies on a worker thread.
pub struct Tessellator<K> {
    latest: Latest<K>,
    next: u64,
    answers: mpsc::Receiver<Done<K>>,
    /// Dropping the sender stops the worker after the job it is on; nothing waits for it.
    #[cfg(not(target_arch = "wasm32"))]
    jobs: mpsc::Sender<Job<K>>,
    #[cfg(target_arch = "wasm32")]
    kernel: Arc<dyn Kernel>,
    #[cfg(target_arch = "wasm32")]
    answered: mpsc::Sender<Done<K>>,
    #[cfg(target_arch = "wasm32")]
    queue: std::collections::VecDeque<Job<K>>,
}

impl<K: Clone + Eq + Hash + Send + 'static> Tessellator<K> {
    /// A tessellator on `kernel`, with its worker thread started.
    pub fn new(kernel: Arc<dyn Kernel>) -> Tessellator<K> {
        let latest: Latest<K> = Arc::default();
        let (answered, answers) = mpsc::channel();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (jobs, queue) = mpsc::channel::<Job<K>>();
            let current = latest.clone();
            std::thread::Builder::new()
                .name("tessellation".into())
                .spawn(move || {
                    for job in queue {
                        if is_latest(&current, &job.key, job.generation)
                            && answered.send(answer(kernel.as_ref(), job)).is_err()
                        {
                            break;
                        }
                    }
                })
                .expect("starting the tessellation thread");
            Tessellator { latest, next: 1, answers, jobs }
        }
        #[cfg(target_arch = "wasm32")]
        Tessellator { latest, next: 1, answers, kernel, answered, queue: Default::default() }
    }

    /// Asks for `body` tessellated at each of `levels`, superseding any earlier request
    /// for `key`. Returns the request's generation.
    pub fn request(&mut self, key: K, body: BodyId, levels: &[TessellationParams]) -> u64 {
        let generation = self.next;
        self.next += 1;
        self.latest.lock().expect("tessellator lock").insert(key.clone(), generation);
        let job = Job { key, generation, body, levels: levels.to_vec() };
        // The worker only stops when the tessellator is dropped.
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.jobs.send(job);
        #[cfg(target_arch = "wasm32")]
        self.queue.push_back(job);
        generation
    }

    /// Withdraws the request for `key`: its answer, if one comes, is dropped.
    pub fn cancel(&mut self, key: &K) {
        self.latest.lock().expect("tessellator lock").remove(key);
    }

    /// The number of keys whose latest request is not answered yet.
    pub fn pending(&self) -> usize {
        self.latest.lock().expect("tessellator lock").len()
    }

    /// The answers that arrived since the last poll, without waiting. Only the latest
    /// request of each key is answered.
    pub fn poll(&mut self) -> Vec<Done<K>> {
        #[cfg(target_arch = "wasm32")]
        self.run_one();
        std::iter::from_fn(|| self.next_settled()).collect()
    }

    /// Waits up to `timeout` for the next answer to a latest request. For command-line
    /// tools and tests; the interface polls instead. In a WebAssembly build it answers
    /// queued requests in place until one is answered.
    pub fn wait(&mut self, timeout: Duration) -> Option<Done<K>> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = timeout;
            loop {
                let more = self.run_one();
                if let Some(done) = self.next_settled() {
                    return Some(done);
                }
                if !more {
                    return None;
                }
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                let left = deadline.checked_duration_since(std::time::Instant::now())?;
                let done = self.answers.recv_timeout(left).ok()?;
                if self.settle(&done) {
                    return Some(done);
                }
            }
        }
    }

    /// The next answer already received that answers a latest request.
    fn next_settled(&mut self) -> Option<Done<K>> {
        while let Ok(done) = self.answers.try_recv() {
            if self.settle(&done) {
                return Some(done);
            }
        }
        None
    }

    /// Answers the oldest queued request that is still the latest of its key. Returns
    /// false when nothing was left to answer.
    #[cfg(target_arch = "wasm32")]
    fn run_one(&mut self) -> bool {
        while let Some(job) = self.queue.pop_front() {
            if is_latest(&self.latest, &job.key, job.generation) {
                let _ = self.answered.send(answer(self.kernel.as_ref(), job));
                return true;
            }
        }
        false
    }

    /// Marks `done`'s key answered if `done` answers its latest request.
    fn settle(&mut self, done: &Done<K>) -> bool {
        let mut latest = self.latest.lock().expect("tessellator lock");
        if latest.get(&done.key) == Some(&done.generation) {
            latest.remove(&done.key);
            true
        } else {
            false
        }
    }
}
