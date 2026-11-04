use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

pub type Timing = Rc<dyn Fn(&str, Duration)>;

pub(crate) struct Timed {
    pub(crate) name: String,
    pub(crate) future: Pin<Box<dyn Future<Output = ()>>>,
    pub(crate) timing: Option<Timing>,
}

impl Future for Timed {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        let started = Instant::now();
        let polled = this.future.as_mut().poll(cx);
        if let Some(timing) = &this.timing {
            timing(&this.name, started.elapsed());
        }
        polled
    }
}

#[derive(Default)]
pub(crate) struct Signal {
    done: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl Signal {
    pub(crate) fn set(&self) {
        self.done.set(true);
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    pub(crate) fn wait(self: Rc<Self>) -> impl Future<Output = ()> {
        std::future::poll_fn(move |cx| {
            if self.done.get() {
                Poll::Ready(())
            } else {
                *self.waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
    }
}

#[derive(Default)]
pub(crate) struct Turns {
    busy: Cell<bool>,
    waiting: RefCell<std::collections::VecDeque<Rc<Signal>>>,
}

pub(crate) struct Turn(Rc<Turns>);

impl Turns {
    pub(crate) async fn take(self: &Rc<Self>) -> Turn {
        if self.busy.replace(true) {
            let signal = Rc::new(Signal::default());
            self.waiting.borrow_mut().push_back(signal.clone());
            signal.wait().await;
        }
        Turn(self.clone())
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let next = self.0.waiting.borrow_mut().pop_front();
        match next {
            Some(next) => next.set(),
            None => self.0.busy.set(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_are_taken_in_order() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let local = tokio::task::LocalSet::new();
        let turns = Rc::new(Turns::default());
        let log = Rc::new(RefCell::new(vec![]));
        let gate = Rc::new(Signal::default());
        for n in 0..3 {
            let (turns, log, gate) = (turns.clone(), log.clone(), gate.clone());
            local.spawn_local(async move {
                let _turn = turns.take().await;
                log.borrow_mut().push(format!("start {n}"));
                if n == 0 {
                    gate.wait().await;
                }
                log.borrow_mut().push(format!("end {n}"));
            });
        }
        local.block_on(&runtime, tokio::task::yield_now());
        assert_eq!(*log.borrow(), vec!["start 0"]);
        gate.set();
        local.block_on(&runtime, async {
            while log.borrow().len() < 6 {
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(*log.borrow(), vec!["start 0", "end 0", "start 1", "end 1", "start 2", "end 2"]);
        assert!(!turns.busy.get());
    }
}
