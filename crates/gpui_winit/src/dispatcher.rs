use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
    thread::{self, JoinHandle, ThreadId},
    time::{Duration, Instant},
};

use gpui::{
    GLOBAL_THREAD_TIMINGS, PlatformDispatcher, Priority, PriorityQueueReceiver,
    PriorityQueueSender, RunnableVariant, TaskTiming, ThreadTaskTimings,
};
use winit::event_loop::EventLoopProxy;

enum WorkerCommand {
    Run(RunnableVariant),
    Shutdown,
}

struct BackgroundPool {
    sender: Arc<PriorityQueueSender<WorkerCommand>>,
    threads: Vec<JoinHandle<()>>,
}

impl BackgroundPool {
    fn new() -> Self {
        let (sender, receiver) = PriorityQueueReceiver::new();
        let sender = Arc::new(sender);
        let thread_count = thread::available_parallelism().map_or(2, |count| count.get().max(2));
        let mut threads = Vec::with_capacity(thread_count);

        for index in 0..thread_count {
            let receiver = receiver.clone();
            match thread::Builder::new()
                .name(format!("WinitWorker-{index}"))
                .spawn(move || {
                    for command in receiver.iter() {
                        match command {
                            WorkerCommand::Run(runnable) => execute_runnable(runnable),
                            WorkerCommand::Shutdown => break,
                        }
                    }
                }) {
                Ok(thread) => threads.push(thread),
                Err(error) => log::error!("failed to create winit worker thread: {error}"),
            }
        }
        drop(receiver);

        Self { sender, threads }
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        if let Err(error) = self.sender.send(priority, WorkerCommand::Run(runnable)) {
            if let WorkerCommand::Run(runnable) = error.0 {
                execute_runnable(runnable);
            }
        }
    }
}

impl Drop for BackgroundPool {
    fn drop(&mut self) {
        for _ in 0..self.threads.len() {
            let _ = self.sender.send(Priority::High, WorkerCommand::Shutdown);
        }
        for thread in self.threads.drain(..) {
            if thread.join().is_err() {
                log::error!("winit worker thread panicked during shutdown");
            }
        }
    }
}

struct ScheduledRunnable {
    deadline: Instant,
    sequence: u64,
    runnable: RunnableVariant,
}

impl PartialEq for ScheduledRunnable {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.sequence == other.sequence
    }
}

impl Eq for ScheduledRunnable {}

impl PartialOrd for ScheduledRunnable {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledRunnable {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

#[derive(Default)]
struct TimerState {
    queue: BinaryHeap<ScheduledRunnable>,
    next_sequence: u64,
    shutdown: bool,
}

struct TimerScheduler {
    state: Arc<(Mutex<TimerState>, Condvar)>,
    worker_sender: Arc<PriorityQueueSender<WorkerCommand>>,
    thread: Option<JoinHandle<()>>,
}

impl TimerScheduler {
    fn new(worker_sender: Arc<PriorityQueueSender<WorkerCommand>>) -> Self {
        let state = Arc::new((Mutex::new(TimerState::default()), Condvar::new()));
        let thread_state = state.clone();
        let thread_sender = worker_sender.clone();
        let thread = match thread::Builder::new()
            .name("WinitTimer".to_string())
            .spawn(move || run_timer(thread_state, thread_sender))
        {
            Ok(thread) => Some(thread),
            Err(error) => {
                log::error!("failed to create winit timer thread: {error}");
                None
            }
        };
        Self {
            state,
            worker_sender,
            thread,
        }
    }

    fn schedule(&self, duration: Duration, runnable: RunnableVariant) {
        if self.thread.is_none() {
            if let Err(error) = self
                .worker_sender
                .send(Priority::Medium, WorkerCommand::Run(runnable))
                && let WorkerCommand::Run(runnable) = error.0
            {
                execute_runnable(runnable);
            }
            return;
        }

        let deadline = Instant::now()
            .checked_add(duration)
            .unwrap_or_else(Instant::now);
        let (lock, condvar) = self.state.as_ref();
        let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
        if state.shutdown {
            drop(state);
            return;
        }
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.wrapping_add(1);
        state.queue.push(ScheduledRunnable {
            deadline,
            sequence,
            runnable,
        });
        condvar.notify_one();
    }
}

impl Drop for TimerScheduler {
    fn drop(&mut self) {
        let (lock, condvar) = self.state.as_ref();
        {
            let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
            state.shutdown = true;
            state.queue.clear();
        }
        condvar.notify_one();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("winit timer thread panicked during shutdown");
        }
    }
}

fn run_timer(
    state: Arc<(Mutex<TimerState>, Condvar)>,
    worker_sender: Arc<PriorityQueueSender<WorkerCommand>>,
) {
    let (lock, condvar) = state.as_ref();
    let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
    loop {
        if state.shutdown {
            return;
        }

        let Some(task) = state.queue.peek() else {
            state = condvar
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
            continue;
        };
        let now = Instant::now();
        if task.deadline > now {
            let timeout = task.deadline.duration_since(now);
            let (next_state, _) = condvar
                .wait_timeout(state, timeout)
                .unwrap_or_else(|error| error.into_inner());
            state = next_state;
            continue;
        }

        let task = state.queue.pop();
        drop(state);
        if let Some(task) = task
            && let Err(error) =
                worker_sender.send(Priority::Medium, WorkerCommand::Run(task.runnable))
            && let WorkerCommand::Run(runnable) = error.0
        {
            execute_runnable(runnable);
        }
        state = lock.lock().unwrap_or_else(|error| error.into_inner());
    }
}

pub(crate) struct WinitDispatcher {
    main_thread_id: ThreadId,
    main_sender: PriorityQueueSender<RunnableVariant>,
    timer: TimerScheduler,
    background: BackgroundPool,
    realtime_thread_id: AtomicUsize,
    proxy: EventLoopProxy,
}

impl WinitDispatcher {
    pub(crate) fn new(
        proxy: EventLoopProxy,
    ) -> (Arc<Self>, PriorityQueueReceiver<RunnableVariant>) {
        let (main_sender, main_receiver) = PriorityQueueReceiver::new();
        let background = BackgroundPool::new();
        let timer = TimerScheduler::new(background.sender.clone());

        (
            Arc::new(Self {
                main_thread_id: thread::current().id(),
                main_sender,
                timer,
                background,
                realtime_thread_id: AtomicUsize::new(0),
                proxy,
            }),
            main_receiver,
        )
    }
}

pub(crate) fn execute_runnable(runnable: RunnableVariant) {
    let start = Instant::now();
    let mut timing = TaskTiming {
        location: runnable.metadata().location,
        start,
        end: None,
    };
    gpui::profiler::add_task_timing(timing);
    runnable.run();
    timing.end = Some(Instant::now());
    gpui::profiler::add_task_timing(timing);
}

impl PlatformDispatcher for WinitDispatcher {
    fn get_all_timings(&self) -> Vec<ThreadTaskTimings> {
        ThreadTaskTimings::convert(&GLOBAL_THREAD_TIMINGS.lock())
    }

    fn get_current_thread_timings(&self) -> ThreadTaskTimings {
        gpui::profiler::get_current_thread_task_timings()
    }

    fn is_main_thread(&self) -> bool {
        thread::current().id() == self.main_thread_id
    }

    fn dispatch(&self, runnable: RunnableVariant, priority: Priority) {
        if priority == Priority::RealtimeAudio {
            self.spawn_realtime(Box::new(move || execute_runnable(runnable)));
        } else {
            self.background.dispatch(runnable, priority);
        }
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, priority: Priority) {
        if let Err(runnable) = self.main_sender.send(priority, runnable) {
            std::mem::forget(runnable);
            return;
        }
        self.proxy.wake_up();
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        self.timer.schedule(duration, runnable);
    }

    fn spawn_realtime(&self, function: Box<dyn FnOnce() + Send>) {
        let id = self
            .realtime_thread_id
            .fetch_add(1, AtomicOrdering::Relaxed);
        let result = thread::Builder::new()
            .name(format!("WinitRealtime-{id}"))
            .spawn(move || {
                // TODO(winit): Winit does not expose native thread-priority controls.
                function();
            });
        if let Err(error) = result {
            log::error!("failed to create winit realtime thread: {error}");
        }
    }
}
