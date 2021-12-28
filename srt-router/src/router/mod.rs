mod args;
mod cancel;
mod gc;
mod publishers;
mod signal;
mod sources;
mod subscribers;

use std::convert::TryInto;

use srt_tokio::{options::*, SrtListener};

use cancel::CancellationHandle;
use gc::GarbageCollection;
use publishers::{IncomingPublishers, Publishers};
use sources::Sources;
use subscribers::{IncomingSubscribers, Subscribers};

pub use args::RouterArgs;
pub use signal::Signal;

pub struct SrtRouter {
    _publishers: Publishers,
    _sources: Sources,
    _subscribers: Subscribers,
    _pub_listener: SrtListener,
    _sub_listener: SrtListener,
    pub_cancel: CancellationHandle,
    sub_cancel: CancellationHandle,
    gc_cancel: CancellationHandle,
}

impl SrtRouter {
    pub async fn bind(
        publish: impl TryInto<SocketAddress>,
        subscribe: impl TryInto<SocketAddress>,
    ) -> Result<SrtRouter, std::io::Error> {
        let (_pub_listener, pub_incoming) = SrtListener::builder().bind(publish).await?;
        let (_sub_listener, sub_incoming) = SrtListener::builder().bind(subscribe).await?;

        let _publishers = Publishers::default();
        let _sources = Sources::default();
        let _subscribers = Subscribers::default();

        let pub_cancel =
            IncomingPublishers::spawn(_publishers.clone(), _sources.clone(), pub_incoming);

        let sub_cancel =
            IncomingSubscribers::spawn(_subscribers.clone(), _sources.clone(), sub_incoming);

        let gc_cancel =
            GarbageCollection::spawn(_publishers.clone(), _sources.clone(), _subscribers.clone());

        Ok(SrtRouter {
            _publishers,
            _sources,
            _subscribers,
            _pub_listener,
            _sub_listener,
            pub_cancel,
            sub_cancel,
            gc_cancel,
        })
    }

    pub async fn close(self) {
        self.sub_cancel.cancel().await;
        self.pub_cancel.cancel().await;
        self.gc_cancel.cancel().await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};
    use log::info;
    use tokio::sync::oneshot;

    use srt_tokio::*;

    use crate::router::SrtRouter;

    #[tokio::test]
    async fn pub_sub() -> Result<(), std::io::Error> {
        let _ = pretty_env_logger::try_init();

        let (finished_send, finished_recv) = oneshot::channel();

        let pub_address = "127.0.0.1:7712";
        let sub_address = "127.0.0.1:7701";

        let router = SrtRouter::bind(pub_address, sub_address).await?;

        let mut sender = SrtSocket::builder()
            .call(pub_address, Some("#!::r=source"))
            .await?;

        let sent = tokio::spawn(async move {
            info!("Sending");
            let mut count = 0;
            while !finished_send.is_closed() {
                let _ = sender.send((Instant::now(), Bytes::from("hello"))).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
                count += 1;
            }
            let _ = sender.close().await;
            let _ = finished_send.send(());
            count
        });

        tokio::time::sleep(Duration::from_millis(200)).await;

        let subscriber = SrtSocket::builder()
            .call(sub_address, Some("#!::r=source"))
            .await?;

        let received = tokio::spawn(async move {
            info!("Sending");
            let mut count = 0;
            let mut subscriber = subscriber.fuse();
            if let Some(Ok(data)) = subscriber.next().await {
                println!("{:?}", data.1);
                count += 1;
            }
            std::mem::drop(finished_recv);
            let _ = subscriber.close().await;
            count
        });

        tokio::time::sleep(Duration::from_millis(200)).await;

        router.close().await;

        assert!(sent.await.unwrap() > 0);
        assert!(received.await.unwrap() > 0);

        Ok(())
    }
}
