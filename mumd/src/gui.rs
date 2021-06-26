use glib::{clone, MainContext};
use gtk::glib;
use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Button};
use mumd::state::server::Server;
use mumlib::command::Command;
use std::sync::Arc;
use tokio::sync::mpsc;

pub fn start(
    command_sender: mpsc::UnboundedSender<Command>,
    server_receiver: mpsc::UnboundedReceiver<Option<Server>>
) {
    let command_sender = Arc::new(command_sender);
    let server_receiver = Arc::new(std::sync::Mutex::new(server_receiver));
    let app = Application::new(Some("net.sornas.mum"), Default::default());
    app.connect_activate(move |app| {
        let window = ApplicationWindow::new(app);
        window.set_title(Some("My GTK App"));

        let button = Button::builder()
            .label("Disconnect")
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        let command_sender = Arc::clone(&command_sender);
        button.connect_clicked(move |_| {
            command_sender.send(Command::ServerDisconnect).unwrap();
        });

        let main_context = MainContext::default();
        let server_receiver = Arc::clone(&server_receiver);
        main_context.spawn_local(clone!(@weak button => async move {
            while let Some(server) = server_receiver.lock().unwrap().recv().await {
                button.set_sensitive(server.is_some());
            }
        }));

        window.set_child(Some(&button));
        window.present();
    });
    app.run();
}
