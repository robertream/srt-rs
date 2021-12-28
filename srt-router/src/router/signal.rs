use tokio::sync::mpsc;

pub struct Signal(mpsc::Receiver<()>);

impl Signal {
    pub fn new() -> Self {
        let (ctrlc_sender, ctrlc_receiver) = mpsc::channel(1);
        let _ = ctrlc::set_handler(move || ctrlc_sender.blocking_send(()).unwrap());
        Self(ctrlc_receiver)
    }

    pub async fn wait(&mut self) {
        self.0.recv().await;
    }
}
