use std::{
    collections::{btree_map, BTreeMap},
    convert::{TryFrom, TryInto},
    mem::replace,
    net::SocketAddr,
    ops::Deref,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use futures::StreamExt;
use itertools::Itertools;
use tokio::{
    select,
    sync::{broadcast, oneshot, RwLock},
};

use srt_tokio::{access::*, options::StreamId, SrtSocket};

use crate::router::cancel::CancellationHandle;

use super::publishers::Publishers;

#[derive(Clone, Default)]
pub struct Sources(Arc<RwLock<BTreeMap<SourceId, Source>>>);

impl Sources {
    pub async fn connect_publisher(
        &self,
        source_id: &SourceId,
        publishers: Publishers,
        socket: SrtSocket,
    ) {
        use btree_map::Entry::*;
        let source = match self.0.write().await.entry(source_id.clone()) {
            Vacant(entry) => entry.insert(Source::new()).clone(),
            Occupied(mut entry) => entry.get_mut().clone(),
        };
        tokio::spawn(async move { source.connect_publisher(publishers, socket).await });
    }

    pub async fn subscribe(&self, source_id: &SourceId) -> Option<SourceSubscription> {
        if let Some(source) = self.0.read().await.get(source_id).cloned() {
            Some((source_id.clone(), source.subscribe().await))
        } else {
            None
        }
    }

    pub async fn collect_expired(&self) {
        let timeout = Duration::from_secs(120);
        let mut sources = self.0.write().await;
        let mut remove = vec![];
        for (source_id, source) in sources.iter() {
            use SourceInput::*;
            match source.0.read().await.0 {
                Disconnected(time) if time + timeout < Instant::now() => {
                    remove.push(source_id.clone())
                }
                _ => {}
            }
        }
        for source_id in remove {
            let _ = sources.remove(&source_id);
        }
    }
}

pub type SourceSender = broadcast::Sender<(Instant, Bytes)>;
pub type SourceReceiver = broadcast::Receiver<(Instant, Bytes)>;
pub type SourceSubscription = (SourceId, SourceReceiver);

#[derive(Clone)]
pub struct Source(Arc<RwLock<(SourceInput, SourceSender, SourceReceiver)>>);

impl Source {
    pub fn new() -> Self {
        let (relay_sender, relay_receiver) = broadcast::channel(10_000);
        let input = SourceInput::Connecting(Instant::now());
        Source(Arc::new(RwLock::new((input, relay_sender, relay_receiver))))
    }

    pub async fn connect_publisher(&self, publishers: Publishers, socket: SrtSocket) {
        use SourceInput::*;
        let mut this = self.0.write().await;
        let remote = socket.settings().remote;

        if let Connected(_, _, handle) = replace(&mut this.0, Connecting(Instant::now())) {
            handle.cancel().await;
        }

        let (cancel, mut canceled) = oneshot::channel();
        let relay = this.1.clone();
        let source = self.clone();
        let handle = tokio::spawn(async move {
            publishers.set_publishing(remote).await;

            let mut input = socket.fuse();
            loop {
                let result = select!(
                    data = input.next() => data,
                    _ = &mut canceled => break,
                );
                let result = match result {
                    Some(Ok(data)) => relay.send(data),
                    _ => break,
                };
                if result.is_err() {
                    break;
                }
            }

            publishers.set_disconnected(remote).await;
            let source_input = &mut source.0.write().await.0;
            if matches!(source_input, Connected(_, _, _)) {
                *source_input = Disconnected(Instant::now());
            }
        });

        this.0 = Connected(Instant::now(), remote, CancellationHandle(cancel, handle));
    }

    pub async fn subscribe(&self) -> broadcast::Receiver<(Instant, Bytes)> {
        self.0.read().await.1.subscribe()
    }
}

enum SourceInput {
    Connecting(Instant),
    Connected(Instant, SocketAddr, CancellationHandle),
    Disconnected(Instant),
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct SourceId(String);

impl Deref for SourceId {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<Option<&StreamId>> for SourceId {
    type Error = ();

    fn try_from(value: Option<&StreamId>) -> Result<Self, Self::Error> {
        value.ok_or(())?.as_str().parse()
    }
}

impl FromStr for SourceId {
    type Err = ();

    fn from_str(stream_id: &str) -> Result<Self, Self::Err> {
        let acl = stream_id.parse::<AccessControlList>().ok().ok_or(())?;
        let filter_resource_name = |entity: AccessControlEntry| match entity.try_into() {
            Ok(StandardAccessControlEntry::ResourceName(name)) => Some(name),
            _ => None,
        };

        let source_id = acl.0.into_iter().filter_map(filter_resource_name).join(",");
        if source_id.is_empty() {
            Err(())
        } else {
            Ok(Self(source_id))
        }
    }
}
