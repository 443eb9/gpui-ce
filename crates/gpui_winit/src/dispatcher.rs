use std::{
    sync::Arc,
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

use gpui::{
    GLOBAL_THREAD_TIMINGS, PlatformDispatcher, Priority, PriorityQueueReceiver,
    PriorityQueueSender, RunnableVariant, TaskTiming, ThreadTaskTimings,
};
use winit::event_loop::EventLoopProxy;

pub(crate) struct WinitDispatcher {
    main_thread_id: ThreadId,
    main_sender: PriorityQueueSender<RunnableVariant>,
    background_sender: PriorityQueueSender<RunnableVariant>,
    proxy: EventLoopProxy,
}

impl WinitDispatcher {
    pub(crate) fn new(
        proxy: EventLoopProxy,
    ) -> (Arc<Self>, PriorityQueueReceiver<RunnableVariant>) {
        let (main_sender, main_receiver) = PriorityQueueReceiver::new();
        let (background_sender, background_receiver) = PriorityQueueReceiver::new();
        let thread_count = thread::available_parallelism().map_or(2, |count| count.get().max(2));

        for index in 0..thread_count {
            let receiver = background_receiver.clone();
            thread::Builder::new()
                .name(format!("WinitWorker-{index}"))
                .spawn(move || {
                    for runnable in receiver.iter() {
                        execute_runnable(runnable);
                    }
                })
                .expect("failed to create winit worker thread");
        }

        (
            Arc::new(Self {
                main_thread_id: thread::current().id(),
                main_sender,
                background_sender,
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
        if let Err(error) = self.background_sender.send(priority, runnable) {
            execute_runnable(error.0);
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
        thread::spawn(move || {
            thread::sleep(duration);
            execute_runnable(runnable);
        });
    }

    fn spawn_realtime(&self, function: Box<dyn FnOnce() + Send>) {
        thread::spawn(function);
    }
}
