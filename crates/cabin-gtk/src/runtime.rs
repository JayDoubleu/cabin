//! Bridges Tokio work to the GTK main loop.
//!
//! The Podman client uses Tokio for its UNIX socket I/O, so its futures must
//! run on a Tokio runtime and cannot run on the GLib main context. GTK objects
//! are not `Send`, so they cannot cross to a Tokio worker thread either.
//!
//! The bridge is a one-shot channel per call. The future runs on Tokio, the
//! result crosses back, and the callback runs on the main thread where it may
//! touch widgets.

use std::future::Future;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        // Two worker threads is ample: the work is a handful of concurrent
        // socket requests, never anything CPU bound.
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("could not start the Tokio runtime")
    })
}

/// Runs `future` on Tokio and calls `on_done` on the GTK main thread.
pub fn spawn<F, T>(future: F, on_done: impl FnOnce(T) + 'static)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (sender, receiver) = async_channel::bounded(1);
    runtime().spawn(async move {
        let _ = sender.send(future.await).await;
    });
    gtk::glib::spawn_future_local(async move {
        if let Ok(value) = receiver.recv().await {
            on_done(value);
        }
    });
}

/// Runs a future that produces many values, calling `on_item` for each one on
/// the GTK main thread. Used for the Podman event feed.
pub fn spawn_stream<F, T>(future: F, mut on_item: impl FnMut(T) + 'static)
where
    F: FnOnce(async_channel::Sender<T>) -> BoxFuture + Send + 'static,
    T: Send + 'static,
{
    // A small buffer: events are tiny and the interface drains them quickly,
    // but a burst during start-up should not block the producer.
    let (sender, receiver) = async_channel::bounded(64);
    runtime().spawn(future(sender));
    gtk::glib::spawn_future_local(async move {
        while let Ok(item) = receiver.recv().await {
            on_item(item);
        }
    });
}

pub type BoxFuture = std::pin::Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
