// use core::time;
use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{
    sync::{Mutex, Notify},
    time,
};

pub trait Message: Send + 'static {}
impl<T: Send + 'static> Message for T {}

pub trait Module: Send + 'static {}
impl<T: Send + 'static> Module for T {}

/// A trait for modules capable of handling messages of type `M`.
#[async_trait::async_trait]
pub trait Handler<M: Message>: Module {
    /// Handles the message. A module must be able to access a `ModuleRef` to itself through `self_ref`.
    async fn handle(&mut self, self_ref: &ModuleRef<Self>, msg: M);
}

/// A handle returned by `ModuleRef::request_tick()`, can be used to stop sending further ticks.
// You can add fields to this struct
pub struct TimerHandle {
    is_active: Arc<Mutex<bool>>,
}

impl TimerHandle {
    /// Stops the sending of ticks resulting from the corresponding call to `ModuleRef::request_tick()`.
    /// If the ticks are already stopped, does nothing.
    pub async fn stop(&self) {
        let mut is_active = self.is_active.lock().await;
        *is_active = false;
        // release the lock happens automatically
    }
}

// You can add fields to this struct.
pub struct System {
    modules: Arc<Mutex<HashMap<u32, Arc<dyn Module>>>>,
    counter: u32,
    request_tick_handles: Arc<Mutex<Vec<Arc<Mutex<bool>>>>>, // more outer mutex is to provide safe adding of moduleRef
    vec_task_handles: Vec<Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>>,
    shutting_down: Arc<Mutex<bool>>,
}

impl System {
    /// Registers the module in the system.
    /// Returns a `ModuleRef`, which can be used then to send messages to the module.
    pub async fn register_module<T: Module>(&mut self, module: T) -> ModuleRef<T> {
        let module = Arc::new(Mutex::new(module));

        // make new task_handles for this module and add it to the vec_task_handles
        let task_handles = Arc::new(Mutex::new(Vec::new()));
        self.vec_task_handles.push(task_handles.clone());

        let module_ref = ModuleRef {
            module: module.clone(),
            timer_handles: self.request_tick_handles.clone(),
            notifier: Arc::new(Mutex::new(Vec::new())),
            task_handles: task_handles.clone(),
            shutting_down: self.shutting_down.clone(),
        };
        let mut modules = self.modules.lock().await;
        modules.insert(self.counter, Arc::new(module) as Arc<dyn Module>);
        self.counter += 1;
        module_ref
    }

    /// Creates and starts a new instance of the system.
    pub async fn new() -> Self {
        System {
            modules: Arc::new(Mutex::new(HashMap::new())),
            counter: 0,
            request_tick_handles: Arc::new(Mutex::new(Vec::new())),
            vec_task_handles: Vec::new(),
            shutting_down: Arc::new(Mutex::new(false)),
        }
    }

    /// Gracefully shuts the system down.
    pub async fn shutdown(&mut self) {
        *self.shutting_down.lock().await = true;

        // wait for all tasks to finish
        for task_handles in self.vec_task_handles.iter() {
            for task_handle in task_handles.lock().await.iter_mut() {
                // it must be mutable
                task_handle.await.unwrap();
            }
        }

        for handler in self.request_tick_handles.lock().await.iter() {
            let mut handler = handler.lock().await;
            *handler = false;
        }
    }
}

/// A reference to a module used for sending messages.
// You can add fields to this struct.
pub struct ModuleRef<T: Module + ?Sized> {
    // A marker field required to inform the compiler about variance in T.
    // It can be removed if type T is used in some other field.
    module: Arc<Mutex<T>>,
    timer_handles: Arc<Mutex<Vec<Arc<Mutex<bool>>>>>, // for stopping request_tick
    notifier: Arc<Mutex<Vec<Arc<Notify>>>>,
    task_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    shutting_down: Arc<Mutex<bool>>,
}

impl<T: Module> ModuleRef<T> {
    /// Sends the message to the module.
    pub async fn send<M: Message>(&self, msg: M)
    where
        T: Handler<M>,
    {
        let self2 = self.clone();
        let to_wait_for = match self.notifier.lock().await.pop() {
            Some(notifier) => notifier,
            None => {
                let new_notify = Arc::new(Notify::new());
                new_notify.notify_one();
                new_notify
            }
        };
        let to_notify = Arc::new(Notify::new());
        self.notifier.lock().await.push(to_notify.clone());

        let task_handle = tokio::task::spawn(async move {
            to_wait_for.notified().await; // wait for the previous message to be handled *
                                          // * - this is actually more subtle. Previous message might not be handled yet,
                                          // but we are sure that it will be handled before the next message, or this next message is
                                          // requested by handler itself
            let mut module = self2.module.lock().await;

            to_notify.notify_one(); // notify the next message that it can be handled, we do this right after
                                    // acquiring the lock, so there is no risk other tak will execute concurrently, but if
                                    // another message is requested by the handler, we need to allow its handling to start,
                                    // hence we notify it right after acquiring the lock

            if *self2.shutting_down.lock().await {
                // if shutting down, don't handle the message
                // and don't do anything else as we already notified the next message
                return;
            }

            // do stuff with module
            module.handle(&self2, msg).await;
        });
        self.task_handles.lock().await.push(task_handle);
    }

    /// Schedules a message to be sent to the module periodically with the given interval.
    /// The first tick is sent after the interval elapses.
    /// Every call to this function results in sending new ticks and does not cancel
    /// ticks resulting from previous calls.
    pub async fn request_tick<M>(&self, message: M, delay: Duration) -> TimerHandle
    where
        M: Message + Clone,
        T: Handler<M>,
    {
        // get current time precisely
        let self2 = self.clone(); // for the closure
        let timer_handle = TimerHandle {
            is_active: Arc::new(Mutex::new(true)),
        };
        let is_active = timer_handle.is_active.clone();
        self.timer_handles.lock().await.push(is_active.clone());

        let mut interval = time::interval(delay); // like sleep but takes into account time spent between .tick()
        interval.tick().await; // first tick doesn't wait

        tokio::task::spawn(async move {
            loop {
                // measure time elapsed
                // let start_time = tokio::time::Instant::now();
                interval.tick().await;
                let is_active = is_active.lock().await;
                if !*is_active {
                    break;
                }
                self2.send(message.clone()).await;
            }
        });
        timer_handle
    }
}

impl<T: Module> Clone for ModuleRef<T> {
    /// Creates a new reference to the same module.
    fn clone(&self) -> Self {
        ModuleRef {
            module: self.module.clone(),
            timer_handles: self.timer_handles.clone(),
            notifier: self.notifier.clone(),
            task_handles: self.task_handles.clone(),
            shutting_down: self.shutting_down.clone(),
        }
    }
}
