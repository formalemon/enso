//! Questions waiting to be asked, one at a time: each is a prompt process
//! (this program, `enso-auth prompt`), started when its turn comes. Asking
//! returns a future, so a D-Bus call can wait for the answer without
//! holding anything up; a question can be taken back while it waits or
//! while it's on screen.

use std::{
    collections::VecDeque,
    future::Future,
    io::{BufRead, BufReader, Write},
    pin::Pin,
    process::{Child, ChildStdin, Stdio},
    sync::{Arc, Condvar, Mutex, PoisonError},
    task::{Context, Poll, Waker},
    thread,
    time::Duration,
};

use crate::request::{self, Answer, Request};

/// Where an answer arrives, and who to wake when it does.
#[derive(Debug, Default)]
struct Slot {
    answer: Option<Answer>,
    waker: Option<Waker>,
}

/// An answer still to come.
#[derive(Debug)]
pub struct Pending(Arc<Mutex<Slot>>);

impl Future for Pending {
    type Output = Answer;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Answer> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.answer.take() {
            Some(answer) => Poll::Ready(answer),
            None => {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

fn settle(slot: &Mutex<Slot>, answer: Answer) {
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    slot.answer = Some(answer);
    if let Some(waker) = slot.waker.take() {
        waker.wake();
    }
}

#[derive(Debug)]
struct Job {
    /// What it can be taken back by: polkit's cookie, or `bluetooth`.
    tag: String,
    request: Request,
    slot: Arc<Mutex<Slot>>,
}

#[derive(Debug, Default)]
struct State {
    waiting: VecDeque<Job>,
    /// The question on screen: its tag, and the way to tell it to go.
    showing: Option<(String, ChildStdin)>,
}

/// The line of questions.
#[derive(Clone, Debug)]
pub struct Prompts {
    shared: Arc<(Mutex<State>, Condvar)>,
}

impl Prompts {
    /// Start asking.
    pub fn start() -> Self {
        let prompts = Self {
            shared: Arc::default(),
        };
        let worker = prompts.clone();
        let _ = thread::Builder::new()
            .name("enso-prompts".to_owned())
            .spawn(move || worker.run());
        prompts
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ask `request` when its turn comes.
    pub fn ask(&self, tag: &str, request: Request) -> Pending {
        let slot = Arc::new(Mutex::new(Slot::default()));
        self.lock().waiting.push_back(Job {
            tag: tag.to_owned(),
            request,
            slot: slot.clone(),
        });
        self.shared.1.notify_all();
        Pending(slot)
    }

    /// Take back every question tagged `tag`: those waiting are answered
    /// "no" at once; the one on screen is put away.
    pub fn cancel(&self, tag: &str) {
        let mut state = self.lock();
        state.waiting.retain(|job| {
            let keep = job.tag != tag;
            if !keep {
                settle(&job.slot, Answer::Deny);
            }
            keep
        });
        if let Some((showing, stdin)) = &mut state.showing
            && showing == tag
        {
            let _ = stdin.write_all(b"close\n");
            let _ = stdin.flush();
        }
    }

    fn run(&self) {
        loop {
            let job = {
                let mut state = self.lock();
                loop {
                    if let Some(job) = state.waiting.pop_front() {
                        break job;
                    }
                    state = self
                        .shared
                        .1
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            let answer = self.show(&job).unwrap_or_else(|err| {
                eprintln!("enso-auth: can't show the prompt: {err}");
                Answer::Deny
            });
            settle(&job.slot, answer);
        }
    }

    /// Show one question and wait for its answer.
    fn show(&self, job: &Job) -> std::io::Result<Answer> {
        // This very program, even after a rebuild has replaced its file.
        let mut child = enso_core::exe::again()
            .arg("prompt")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("no stdout"))?;
        request::write(&job.request, &mut stdin)?;
        self.lock().showing = Some((job.tag.clone(), stdin));
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line);
        // Whatever happened, it's no longer on screen.
        self.lock().showing = None;
        reap(child);
        read?;
        Ok(Answer::parse(&line))
    }
}

/// Let the prompt fade out and exit; one that doesn't is stopped.
fn reap(mut child: Child) {
    thread::spawn(move || {
        for _ in 0..40 {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    });
}
