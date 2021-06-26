use mumlib::command::Command;
use tokio::sync::mpsc;

pub fn start(_: mpsc::UnboundedSender<Command>) {
    loop {
        println!("gui");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
