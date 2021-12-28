use argh::FromArgs;
use srt_tokio::options::SocketAddress;

#[derive(FromArgs)]
/// SRT router
pub struct RouterArgs {
    #[argh(positional, short = 'p')]
    /// publisher address and port
    pub publish: SocketAddress,

    #[argh(positional, short = 's')]
    /// subscriber address and port
    pub subscribe: SocketAddress,
}
