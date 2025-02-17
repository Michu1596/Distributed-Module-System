# Distributed-Module-System
## Module system specification


The module system interface provides the following functionality:

- Creating and starting new instances of the system (System::new()).

Registering modules in the system (System::register_module()). The Module trait specifies bounds that a module must satisfy. Registering a module yields a ModuleRef, which can be used to send messages to the module.

- Sending messages to registered modules (ModuleRef::send()). A message of type M can be sent to a module of type T if T implements the Handler<M> trait. A module should handle messages in the order in which it receives them.

- A message is considered as delivered after the corresponding ModuleRef::send() has finished. In other words, the system must behave as if ModuleRef::send() inserted a message at the end of the receiving module’s message queue.

- Creating new references to registered modules (<ModuleRef as Clone>::clone()).

- Scheduling a message to be sent to a registered module periodically with a given interval (ModuleRef::request_tick()). The first tick should be sent after the interval elapsed. Requesting a tick yields a TimerHandle, which can be used to stop the sending of further ticks resulting from this request (TimerHandle::stop()).

- ModuleRef::request_tick() can be called multiple times. Every call sends more ticks and does not cancel ticks resulting from previous calls. For example, if ModuleRef::request_tick() is called at time 0 with interval 2 and at time 1 with interval 3, ticks should arrive at times 2, 4, 4 (two ticks at time 4), 6, 7, …

- Shutting the system down gracefully (System::shutdown()). The shutdown should wait for all already started handlers to finish and for all registered modules to be dropped. It should not wait for all enqueued messages to be handled. It does not have to wait for all Tokio tasks to finish, but it must cause all of them to finish (e.g., it is acceptable if a task handling ModuleRef::request_tick() finishes an interval after the shutdown).
