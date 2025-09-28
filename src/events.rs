use uuid::Uuid;

use {
    futures::Stream,
    std::{
        collections::HashMap,
        fmt::Debug,
        ops::Deref,
        pin::Pin,
        sync::{Arc, RwLock},
        task::{Context, Poll},
    },
    tokio::sync::{
        Mutex,
        mpsc::{self, UnboundedReceiver, unbounded_channel},
    },
    tracing::instrument,
};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct EventTarget<T: Debug> {
    listeners: Arc<RwLock<HashMap<Uuid, Arc<Subscription<T>>>>>,
    sender: Arc<mpsc::UnboundedSender<Arc<T>>>,
    receiver: Arc<Mutex<mpsc::UnboundedReceiver<Arc<T>>>>,
}

impl<T: Debug> EventTarget<T> {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        Self {
            listeners: Arc::new(RwLock::new(HashMap::new())),
            sender: sender.into(),
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }

    #[instrument(level = "trace")]
    pub fn emit(&self, v: impl Into<Arc<T>> + Debug) {
        let v = v.into();

        // Notify all listeners
        if let Ok(listeners) = self.listeners.read() {
            listeners.values().for_each(|s| s.update(v.clone()));
        }

        // Send to stream (ignore error if receiver is dropped)
        let _ = self.sender.send(v);
    }

    pub fn on(&self, handler: impl Fn(Arc<T>) + Send + Sync + 'static) -> Arc<Subscription<T>> {
        let sub = Arc::new(Subscription::new(self, handler));
        if let Ok(mut listeners) = self.listeners.write() {
            listeners.insert(sub.id, sub.clone());
        }
        sub
    }

    pub fn off(&self, sub: &Subscription<T>) {
        if let Ok(mut listeners) = self.listeners.write() {
            listeners.remove(&sub.id);
        }
    }

    pub fn as_stream(&self) -> EventStream<T>
    where
        T: Send + Sync + 'static,
    {
        EventStream::new(self)
    }
}

impl<T: Debug> Default for EventTarget<T> {
    fn default() -> Self { Self::new() }
}

pub struct Subscription<T: Debug> {
    id: Uuid,
    handler: Box<dyn Fn(Arc<T>) + Send + Sync>,
    to: *const EventTarget<T>, // Using raw pointer to avoid lifetime issues
}

impl<T: Debug> Debug for Subscription<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription").field("id", &self.id).field("handler", &"<function>").field("to", &self.to).finish()
    }
}

unsafe impl<T: Debug> Send for Subscription<T> {}
unsafe impl<T: Debug> Sync for Subscription<T> {}

impl<T: Debug> Subscription<T> {
    pub fn new(to: &EventTarget<T>, handler: impl Fn(Arc<T>) + Send + Sync + 'static) -> Self {
        Self { id: Uuid::new_v4(), handler: Box::new(handler), to: to as *const _ }
    }

    pub fn off(&self) {
        unsafe {
            if let Some(target) = self.to.as_ref() {
                target.off(self);
            }
        }
    }

    #[instrument(level = "trace")]
    pub(crate) fn update(&self, v: Arc<T>) { (self.handler)(v) }
}

impl<T: Debug> Drop for Subscription<T> {
    fn drop(&mut self) {
        unsafe {
            self.to.read().off(self);
        }
    }
}

#[allow(dead_code)]
pub struct EventStream<T: Debug> {
    sub: Arc<Subscription<T>>,
    ch: UnboundedReceiver<Arc<T>>,
}

impl<T: Debug> EventStream<T>
where
    T: Send + Sync + 'static,
{
    pub fn new(et: &EventTarget<T>) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            ch: rx,
            sub: et.on(move |v| {
                let _ = tx.send(v);
            }),
        }
    }
}

impl<T: Debug> Deref for EventStream<T> {
    type Target = UnboundedReceiver<Arc<T>>;

    fn deref(&self) -> &Self::Target { &self.ch }
}

impl<T: Debug> Stream for EventStream<T> {
    type Item = Arc<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> { self.ch.poll_recv(cx) }
}
