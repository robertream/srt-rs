use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::{sync::oneshot, task::JoinHandle};

pub struct CancellationHandle(pub oneshot::Sender<()>, pub JoinHandle<()>);

impl CancellationHandle {
    pub async fn cancel(self) {
        let _ = self.0.send(());
        let _ = self.1.await;
    }
}

impl Future for CancellationHandle {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.get_mut().1).poll(cx) {
            Poll::Ready(_) => Poll::Ready(()),
            Poll::Pending => Poll::Pending,
        }
    }
}
