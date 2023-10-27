use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A collection of event listeners for a task.
#[derive(Clone, Debug)]
pub struct EventListeners<T> {
    /// All listeners for events
    listeners: Vec<mpsc::UnboundedSender<T>>,
}

impl<T> Default for EventListeners<T> {
    fn default() -> Self {
        Self { listeners: Vec::new() }
    }
}

impl<T: Clone> EventListeners<T> {
    /// Send an event to all listeners.
    /// 发送一个event到所有的listeners
    ///
    /// Channels that were closed are removed.
    /// Channels在被移除之后关闭
    pub fn notify(&mut self, event: T) {
        self.listeners.retain(|listener| listener.send(event.clone()).is_ok())
    }

    /// Add a new event listener.
    /// 添加一个新的event listener
    pub fn new_listener(&mut self) -> UnboundedReceiverStream<T> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.listeners.push(sender);
        UnboundedReceiverStream::new(receiver)
    }

    /// Push new event listener.
    /// push一个新的event listener
    pub fn push_listener(&mut self, listener: mpsc::UnboundedSender<T>) {
        self.listeners.push(listener);
    }
}
