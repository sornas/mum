use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Button};
use mumlib::command::Command;
use std::sync::Arc;
use tokio::sync::mpsc;

pub fn start(command_sender: mpsc::UnboundedSender<Command>) {
    let command_sender = Arc::new(command_sender);
    let app = Application::new(Some("net.sornas.mum"), Default::default());
    app.connect_activate(move |app| {
        let command_sender = Arc::clone(&command_sender);
        build_ui(app, move |_button| {
            command_sender.send(Command::ServerDisconnect).unwrap();
        })
    });
    app.run();
}

fn build_ui<F: Fn(&Button) + 'static>(app: &Application, on_click: F) {
    let window = ApplicationWindow::new(app);
    window.set_title(Some("My GTK App"));

    let button = Button::with_label("Disconnect");
    button.set_margin_top(12);
    button.set_margin_bottom(12);
    button.set_margin_start(12);
    button.set_margin_end(12);

    button.connect_clicked(on_click);

    window.set_child(Some(&button));
    window.present();
}
