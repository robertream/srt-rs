use std::time::Duration;

use futures::{select, FutureExt};
use tokio::sync::oneshot;

use super::{
    cancel::CancellationHandle, publishers::Publishers, sources::Sources, subscribers::Subscribers,
};

pub struct GarbageCollection {
    publishers: Publishers,
    sources: Sources,
    subscribers: Subscribers,
}

impl GarbageCollection {
    pub fn spawn(
        publishers: Publishers,
        sources: Sources,
        subscribers: Subscribers,
    ) -> CancellationHandle {
        let (cancel, canceled) = oneshot::channel();
        let this = GarbageCollection {
            publishers,
            sources,
            subscribers,
        };
        let handle = tokio::spawn(this.run_loop(canceled));
        CancellationHandle(cancel, handle)
    }

    async fn run_loop(self, canceled: oneshot::Receiver<()>) {
        let mut canceled = canceled.fuse();
        loop {
            select!(
                _ = tokio::time::sleep(Duration::from_secs(1)).fuse() => (),
                _ = canceled => break
            );
            self.publishers.collect_expired().await;
            self.sources.collect_expired().await;
            self.subscribers.collect_expired().await;
        }
    }
}
