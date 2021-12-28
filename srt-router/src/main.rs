mod router;

use router::*;

#[tokio::main]
async fn main() {
    let args: RouterArgs = argh::from_env();
    let router = SrtRouter::bind(args.publish, args.subscribe).await.unwrap();

    let mut signal = Signal::new();
    signal.wait().await;

    use futures::FutureExt;
    tokio::select!(
        _ = router.close().fuse() => (),
        _ = signal.wait().fuse() => ()
    )
}
