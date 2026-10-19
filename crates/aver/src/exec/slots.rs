//! The executor's run slots (§2.4): at most [`ExecConfig::max_sandboxes`]
//! runs at once, shared fairly between the tasks asking. Each requester (a
//! task, for verification) has its own FIFO queue, and a freed slot goes
//! to the next requester in round-robin order, so one task's six runs do
//! not make another task's single run wait for all of them. A run's
//! timeout starts when it gets its slot, never while it waits, so a queued
//! run cannot end `infra`. Within a requester's queue a lower rank goes
//! first, FIFO among equal ranks: in place, the sides of a verification
//! wait in the order that swaps the checkout least.
//!
//! [`ExecConfig::max_sandboxes`]: super::ExecConfig::max_sandboxes

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::oneshot;

/// What a waiting run hears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotEvent {
    /// Waiting, with this many runs to be served first.
    Queued { ahead: u32 },
    /// It has a slot and starts now.
    Started,
}

/// Called with a run's [`SlotEvent`]s.
pub type SlotWatch = Arc<dyn Fn(SlotEvent) + Send + Sync>;

/// How often a waiting run recounts the runs ahead of it.
const RECOUNT: Duration = Duration::from_millis(500);

/// The requester of runs that name none.
pub const ANONYMOUS: &str = "";

struct Waiter {
    ticket: u64,
    rank: u8,
    wake: oneshot::Sender<()>,
}

#[derive(Default)]
struct State {
    free: usize,
    queues: BTreeMap<Arc<str>, VecDeque<Waiter>>,
    /// Requesters with waiters, in the order they are served.
    turn: VecDeque<Arc<str>>,
    next_ticket: u64,
}

impl State {
    /// Runs served before `ticket` of `who`, in round-robin order.
    fn ahead(&self, who: &str, ticket: u64) -> u32 {
        let Some(mine) = self.queues.get(who) else {
            return 0;
        };
        let Some(at) = mine.iter().position(|w| w.ticket == ticket) else {
            return 0;
        };
        let mut ahead = at;
        // Each round serves one waiter of each requester in turn; `who` is
        // served in round `at`, after the requesters before it in turn.
        let me = self.turn.iter().position(|r| &**r == who).unwrap_or(0);
        for (i, r) in self.turn.iter().enumerate() {
            if &**r == who {
                continue;
            }
            let len = self.queues.get(r).map_or(0, VecDeque::len);
            ahead += len.min(if i < me { at + 1 } else { at });
        }
        u32::try_from(ahead).unwrap_or(u32::MAX)
    }

    /// Hands a free slot to the next waiter in turn; false when none waits.
    fn hand_over(&mut self) -> bool {
        while let Some(who) = self.turn.pop_front() {
            let queue = self
                .queues
                .get_mut(&who)
                .expect("a requester in turn has a queue");
            let Some(waiter) = queue.pop_front() else {
                self.queues.remove(&who);
                continue;
            };
            if queue.is_empty() {
                self.queues.remove(&who);
            } else {
                self.turn.push_back(who);
            }
            // A waiter whose run was dropped no longer listens.
            if waiter.wake.send(()).is_ok() {
                return true;
            }
        }
        false
    }
}

/// The slots, shared by every clone of an executor.
pub struct Slots {
    state: Mutex<State>,
}

/// A held slot; dropping it hands it to the next waiter.
pub struct Slot {
    slots: Arc<Slots>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut state = self.slots.state.lock();
        if !state.hand_over() {
            state.free += 1;
        }
    }
}

impl Slots {
    pub fn new(n: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                free: n.max(1),
                ..State::default()
            }),
        })
    }

    /// Runs waiting, all requesters together.
    pub fn waiting(&self) -> usize {
        self.state.lock().queues.values().map(VecDeque::len).sum()
    }

    /// A slot for a run of `who`, in its turn, ahead of `who`'s waiters of a
    /// higher `rank`. `watch` hears how many runs are ahead while it waits
    /// (when that changes) and when it starts.
    pub async fn acquire(self: &Arc<Self>, who: &str, rank: u8, watch: Option<&SlotWatch>) -> Slot {
        let (ticket, mut rx) = {
            let mut state = self.state.lock();
            if state.free > 0 && state.queues.is_empty() {
                state.free -= 1;
                drop(state);
                if let Some(watch) = watch {
                    watch(SlotEvent::Started);
                }
                return Slot {
                    slots: self.clone(),
                };
            }
            let ticket = state.next_ticket;
            state.next_ticket += 1;
            let (tx, rx) = oneshot::channel();
            let who: Arc<str> = Arc::from(who);
            let queue = state.queues.entry(who.clone()).or_default();
            if queue.is_empty() {
                state.turn.push_back(who.clone());
            }
            let queue = state.queues.get_mut(&who).expect("just inserted");
            let at = queue
                .iter()
                .position(|w| w.rank > rank)
                .unwrap_or(queue.len());
            queue.insert(
                at,
                Waiter {
                    ticket,
                    rank,
                    wake: tx,
                },
            );
            (ticket, rx)
        };
        let mut said = None;
        loop {
            let ahead = self.state.lock().ahead(who, ticket);
            if said != Some(ahead) {
                said = Some(ahead);
                if let Some(watch) = watch {
                    watch(SlotEvent::Queued { ahead });
                }
            }
            tokio::select! {
                woke = &mut rx => {
                    woke.expect("a waiter's sender is used or dropped only under the lock");
                    break;
                }
                _ = tokio::time::sleep(RECOUNT) => {}
            }
        }
        if let Some(watch) = watch {
            watch(SlotEvent::Started);
        }
        Slot {
            slots: self.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task A queues six runs on one slot, then task B one: B's run starts
    /// second among the waiters, before A's third, let alone its sixth.
    #[tokio::test]
    async fn a_single_run_of_another_task_goes_before_the_sixth() {
        let slots = Slots::new(1);
        let order = Arc::new(Mutex::new(Vec::new()));
        let first = slots.acquire("A", 0, None).await;
        let mut tasks = tokio::task::JoinSet::new();
        for (who, n) in [("A", 2), ("A", 3), ("A", 4), ("A", 5), ("A", 6), ("B", 1)] {
            let (mine, order) = (slots.clone(), order.clone());
            tasks.spawn(async move {
                let _slot = mine.acquire(who, 0, None).await;
                order.lock().push(format!("{who}{n}"));
                tokio::time::sleep(Duration::from_millis(5)).await;
            });
            // Queue them in this order.
            while slots.waiting() < n_waiting(who, n) {
                tokio::task::yield_now().await;
            }
        }
        assert_eq!(
            slots.state.lock().ahead("B", 5),
            1,
            "only A2 is ahead of B1"
        );
        drop(first);
        while tasks.join_next().await.is_some() {}
        assert_eq!(*order.lock(), ["A2", "B1", "A3", "A4", "A5", "A6"]);
    }

    /// Defect: in place, a head run queued before a base rerun starts
    /// first, so the checkout swaps to the head and back.
    #[tokio::test]
    async fn a_lower_rank_goes_first_within_a_requester() {
        let slots = Slots::new(1);
        let order = Arc::new(Mutex::new(Vec::new()));
        let first = slots.acquire("A", 0, None).await;
        let mut tasks = tokio::task::JoinSet::new();
        for (i, (name, rank)) in [("head", 2), ("head2", 2), ("transplant", 1), ("base", 0)]
            .into_iter()
            .enumerate()
        {
            let (mine, order) = (slots.clone(), order.clone());
            tasks.spawn(async move {
                let _slot = mine.acquire("A", rank, None).await;
                order.lock().push(name);
            });
            while slots.waiting() < i + 1 {
                tokio::task::yield_now().await;
            }
        }
        drop(first);
        while tasks.join_next().await.is_some() {}
        assert_eq!(*order.lock(), ["base", "transplant", "head", "head2"]);
    }

    fn n_waiting(who: &str, n: usize) -> usize {
        if who == "B" {
            6
        } else {
            n - 1
        }
    }

    /// A waiting run hears how many are ahead, and when it starts; a
    /// dropped waiter gives its turn to the next.
    #[tokio::test]
    async fn waiters_hear_their_place_and_dropped_ones_are_skipped() {
        let slots = Slots::new(1);
        let held = slots.acquire("A", 0, None).await;
        let heard = Arc::new(Mutex::new(Vec::new()));
        let watch: SlotWatch = {
            let heard = heard.clone();
            Arc::new(move |e| heard.lock().push(e))
        };
        let gone = {
            let slots = slots.clone();
            tokio::spawn(async move {
                let _ = slots.acquire("A", 0, None).await;
            })
        };
        while slots.waiting() < 1 {
            tokio::task::yield_now().await;
        }
        let waiting = {
            let slots = slots.clone();
            tokio::spawn(async move {
                let _slot = slots.acquire("B", 0, Some(&watch)).await;
            })
        };
        while slots.waiting() < 2 {
            tokio::task::yield_now().await;
        }
        gone.abort();
        let _ = gone.await;
        drop(held);
        waiting.await.unwrap();
        let heard = heard.lock().clone();
        assert_eq!(
            heard.first(),
            Some(&SlotEvent::Queued { ahead: 1 }),
            "{heard:?}"
        );
        assert_eq!(heard.last(), Some(&SlotEvent::Started), "{heard:?}");
        assert_eq!(slots.state.lock().free, 1, "every slot came back");
    }
}
