use std::{
    collections::BTreeMap, convert::TryInto, net::SocketAddr, sync::Arc, time::Duration,
    time::Instant,
};

use futures::{select, FutureExt, SinkExt, StreamExt};
use log::{info, warn};
use srt_tokio::{ConnectionRequest, SrtIncoming, SrtSocket};
use tokio::sync::{oneshot, RwLock};

use super::{
    cancel::CancellationHandle,
    sources::{SourceId, SourceSubscription, Sources},
};

pub struct IncomingSubscribers {
    subscribers: Subscribers,
    sources: Sources,
}

impl IncomingSubscribers {
    pub fn spawn(
        subscribers: Subscribers,
        sources: Sources,
        incoming: SrtIncoming,
    ) -> CancellationHandle {
        let (cancel, canceled) = oneshot::channel();
        let this = IncomingSubscribers {
            subscribers,
            sources,
        };
        let handle = tokio::spawn(this.run_loop(incoming, canceled));
        CancellationHandle(cancel, handle)
    }

    async fn run_loop(self, mut incoming: SrtIncoming, canceled: oneshot::Receiver<()>) {
        let mut incoming = incoming.incoming().fuse();
        let mut canceled = canceled.fuse();
        while let Some(request) = select!(
            request = incoming.next() => request,
            _ = canceled => return)
        {
            self.handle_subscriber_request(request).await
        }
    }

    async fn handle_subscriber_request(&self, request: ConnectionRequest) {
        let subscribers = self.subscribers.clone();
        let sources = self.sources.clone();
        tokio::spawn(async move {
            let remote = request.remote();
            subscribers.set_connecting(remote).await;
            let stream_id = request.stream_id();
            info!("subscriber request: {:?} {:?}", remote, stream_id);
            match stream_id.try_into().ok() {
                None => {
                    subscribers.set_rejected(remote).await;
                }
                Some(source_id) => match sources.subscribe(&source_id).await {
                    None => {
                        let _ = subscribers.set_rejected(remote).await;
                        subscribers.set_rejected(remote).await;
                    }
                    Some(subscription) => match request.accept(None).await {
                        Ok(socket) => {
                            info!("subscriber accepted: {:?} {:?}", remote, subscription.0);
                            subscribers.spawn_subscription(socket, subscription).await;
                        }
                        Err(error) => {
                            warn!(
                                "subscriber error: {:?} {:?} {}",
                                remote, subscription.0, error
                            );
                            subscribers.set_disconnected(remote).await;
                        }
                    },
                },
            }
        });
    }
}

#[derive(Clone, Default)]
pub struct Subscribers(Arc<RwLock<BTreeMap<SocketAddr, Subscriber>>>);

impl Subscribers {
    pub async fn set_connecting(&self, remote: SocketAddr) {
        println!("subscriber connecting: {:?}", remote);
        let _ = self
            .0
            .write()
            .await
            .insert(remote, Subscriber::Connecting(Instant::now()));
    }

    pub async fn set_disconnected(&self, remote: SocketAddr) {
        println!("subscriber disconnected: {:?}", remote);
        let _ = self
            .0
            .write()
            .await
            .insert(remote, Subscriber::Disconnected(Instant::now()));
    }

    pub async fn set_rejected(&self, remote: SocketAddr) {
        println!("subscriber rejected: {:?}", remote);
        let _ = self
            .0
            .write()
            .await
            .insert(remote, Subscriber::Rejected(Instant::now()));
    }

    pub async fn set_connected(
        &self,
        remote: SocketAddr,
        source_id: SourceId,
        cancel: CancellationHandle,
    ) {
        info!("subscriber connected: {:?} {:?}", remote, source_id);
        let _ = self.0.write().await.insert(
            remote,
            Subscriber::Connected(Instant::now(), source_id, cancel),
        );
    }

    pub async fn spawn_subscription(&self, socket: SrtSocket, subscription: SourceSubscription) {
        let remote = socket.settings().remote;
        let subscribers = self.clone();

        let (cancel, canceled) = oneshot::channel();
        let (source_id, mut source) = subscription;
        let mut canceled = canceled.fuse();
        let mut subscriber = socket;
        let handle = tokio::spawn(async move {
            loop {
                use tokio::sync::broadcast::error::RecvError::*;
                let result = select!(
                    data = source.recv().fuse() => data,
                    _ = &mut canceled => Err(Closed),
                );
                match result {
                    Ok(data) => {
                        if subscriber.send(data).await.is_err() {
                            break;
                        }
                    }
                    Err(Closed) => {
                        break;
                    }
                    Err(Lagged(dropped)) => {
                        warn!("Subscriber dropped packets {}", dropped)
                    }
                }
            }
            let _ = subscriber.close().await;

            subscribers.set_disconnected(remote).await;
        });

        let handle = CancellationHandle(cancel, handle);

        self.set_connected(remote, source_id, handle).await;
    }

    pub async fn collect_expired(&self) {
        let mut subscribers = self.0.write().await;
        let remove = subscribers
            .iter()
            .filter_map(|(remote, subscriber)| {
                let timeout = Duration::from_secs(10);
                use Subscriber::*;
                match subscriber {
                    Disconnected(disconnected) | Rejected(disconnected)
                        if *disconnected + timeout < Instant::now() =>
                    {
                        Some(*remote)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        for remote in remove {
            let _ = subscribers.remove(&remote);
        }
    }
}

pub enum Subscriber {
    Connecting(Instant),
    Connected(Instant, SourceId, CancellationHandle),
    Rejected(Instant),
    Disconnected(Instant),
}
