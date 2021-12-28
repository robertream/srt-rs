use std::{
    collections::BTreeMap,
    convert::TryInto,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{select, FutureExt, StreamExt};
use log::{info, warn};
use tokio::sync::{oneshot, RwLock};

use srt_tokio::{ConnectionRequest, SrtIncoming};

use super::{cancel::CancellationHandle, sources::Sources};

pub struct IncomingPublishers {
    publishers: Publishers,
    sources: Sources,
}

impl IncomingPublishers {
    pub fn spawn(
        publishers: Publishers,
        sources: Sources,
        incoming: SrtIncoming,
    ) -> CancellationHandle {
        let (cancel, canceled) = oneshot::channel();
        let this = IncomingPublishers {
            publishers,
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
            self.handle_publisher_request(request).await
        }
    }

    async fn handle_publisher_request(&self, request: ConnectionRequest) {
        let publishers = self.publishers.clone();
        let sources = self.sources.clone();
        tokio::spawn(async move {
            let remote = request.remote();
            publishers.set_connecting(remote).await;
            let stream_id = request.stream_id();
            info!("publisher request: {:?} {:?}", remote, stream_id);
            match stream_id.try_into().ok() {
                Some(source_id) => match request.accept(None).await {
                    Ok(socket) => {
                        info!("publisher accepted: {:?} {:?}", remote, source_id);
                        sources
                            .connect_publisher(&source_id, publishers, socket)
                            .await;
                    }
                    Err(error) => {
                        warn!("publisher error: {:?} {}", remote, error);
                        publishers.set_disconnected(remote).await;
                    }
                },
                None => {
                    publishers.set_rejected(remote).await;
                }
            };
        });
    }
}

#[derive(Clone)]
pub enum Publisher {
    Connecting(Instant),
    Publishing(Instant),
    Rejected(Instant),
    Disconnected(Instant),
}

#[derive(Clone, Default)]
pub struct Publishers(Arc<RwLock<BTreeMap<SocketAddr, Publisher>>>);

impl Publishers {
    pub async fn set_connecting(&self, remote: SocketAddr) {
        info!("publisher connecting: {:?}", remote);
        let _ = self
            .0
            .write()
            .await
            .insert(remote, Publisher::Connecting(Instant::now()));
    }

    pub async fn set_disconnected(&self, remote: SocketAddr) {
        info!("publisher disconnected: {:?}", remote);
        let _ = self
            .0
            .write()
            .await
            .insert(remote, Publisher::Disconnected(Instant::now()));
    }

    pub async fn set_rejected(&self, remote: SocketAddr) {
        info!("publisher rejected: {:?}", remote);
        let _ = self
            .0
            .write()
            .await
            .insert(remote, Publisher::Rejected(Instant::now()));
    }

    pub async fn set_publishing(&self, remote: SocketAddr) {
        info!("publisher publishing: {:?}", remote);
        let _ = self
            .0
            .write()
            .await
            .insert(remote, Publisher::Publishing(Instant::now()));
    }

    pub async fn collect_expired(&self) {
        let mut publishers = self.0.write().await;
        let keys = publishers
            .iter()
            .filter_map(|(remote, publisher)| {
                let timeout = Duration::from_secs(60);
                use Publisher::*;
                match publisher {
                    Disconnected(disconnected) | Rejected(disconnected)
                        if *disconnected + timeout < Instant::now() =>
                    {
                        Some(*remote)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        for key in keys {
            let _ = publishers.remove(&key);
        }
    }
}
