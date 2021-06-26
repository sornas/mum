#[cfg(feature = "gui")]
mod gui;

use mumd::state::{server::Server, State};

use bytes::{BufMut, BytesMut};
use futures_util::{select, FutureExt, SinkExt, StreamExt};
use log::*;
use mumlib::command::{Command, CommandResponse};
use mumlib::setup_logger;
use std::io::ErrorKind;
use tokio::join;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

type CommandSender = mpsc::UnboundedSender<(
    Command,
    mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
)>;

fn main() {
    if std::env::args().any(|s| s.as_str() == "--version" || s.as_str() == "-V") {
        println!("mumd {}", env!("VERSION"));
        return;
    }

    setup_logger(std::io::stderr(), true);

    #[allow(unused_variables)]
    let (gui_cmd_rx, gui_server_tx) = {
        // Scoped allow(unused_variables) so we don't ignore all unusued variables if we
        // compile without feature = gui.

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (server_tx, server_rx) = mpsc::unbounded_channel();
        #[cfg(feature = "gui")]
        std::thread::spawn(move || {
            gui::start(cmd_tx, server_rx);
        });
        (cmd_rx, server_tx)
    };


    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(mumd(gui_cmd_rx, gui_server_tx));
}

async fn mumd(
    gui_command_receiver: mpsc::UnboundedReceiver<Command>,
    gui_server_sender: mpsc::UnboundedSender<Option<Server>>,
) {
    mumd::notifications::init();

    // check if another instance is live
    let connection = UnixStream::connect(mumlib::SOCKET_PATH).await;
    match connection {
        Ok(stream) => {
            let (reader, writer) = stream.into_split();
            let mut reader = FramedRead::new(reader, LengthDelimitedCodec::new());
            let mut writer = FramedWrite::new(writer, LengthDelimitedCodec::new());
            let mut command = BytesMut::new();
            bincode::serialize_into((&mut command).writer(), &Command::Ping).unwrap();
            if let Ok(()) = writer.send(command.freeze()).await {
                if let Some(Ok(buf)) = reader.next().await {
                    if let Ok(Ok::<Option<CommandResponse>, mumlib::Error>(Some(
                        CommandResponse::Pong,
                    ))) = bincode::deserialize(&buf)
                    {
                        error!("Another instance of mumd is already running");
                        return;
                    }
                }
            }
            debug!("a dead socket was found, removing");
            tokio::fs::remove_file(mumlib::SOCKET_PATH).await.unwrap();
        }
        Err(e) => {
            if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused) {
                debug!("a dead socket was found, removing");
                tokio::fs::remove_file(mumlib::SOCKET_PATH).await.unwrap();
            }
        }
    }

    let (command_sender, command_receiver) = mpsc::unbounded_channel();

    let state = match State::new(gui_server_sender) {
        Ok(s) => s,
        Err(e) => {
            error!("Error instantiating mumd: {}", e);
            return;
        }
    };

    // This combination of select/join ensures that we're done if _either_
    // 1) the mumble client terminates, or
    // 2) _both_ the command and gui handler returns.
    let run = select! {
        r = mumd::client::handle(state, command_receiver).fuse() => r,
        // Join already awaits but the select also wants to await so we
        // create a new async block.
        _ = async {
            join!(
                receive_commands(command_sender.clone()).fuse(),
                receive_gui(gui_command_receiver, command_sender).fuse(),
            )
        }.fuse() => Ok(()),
    };

    if let Err(e) = run {
        error!("mumd: {}", e);
        std::process::exit(1);
    }
}

async fn receive_commands(command_sender: CommandSender) {
    let socket = UnixListener::bind(mumlib::SOCKET_PATH).unwrap();

    loop {
        if let Ok((incoming, _)) = socket.accept().await {
            let sender = command_sender.clone();
            tokio::spawn(async move {
                let (reader, writer) = incoming.into_split();
                let mut reader = FramedRead::new(reader, LengthDelimitedCodec::new());
                let mut writer = FramedWrite::new(writer, LengthDelimitedCodec::new());

                while let Some(next) = reader.next().await {
                    let buf = match next {
                        Ok(buf) => buf,
                        Err(_) => continue,
                    };

                    let command = match bincode::deserialize::<Command>(&buf) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    let (tx, mut rx) = mpsc::unbounded_channel();

                    sender.send((command, tx)).unwrap();

                    while let Some(response) = rx.recv().await {
                        let mut serialized = BytesMut::new();
                        bincode::serialize_into((&mut serialized).writer(), &response).unwrap();

                        if let Err(e) = writer.send(serialized.freeze()).await {
                            if e.kind() != ErrorKind::BrokenPipe {
                                //if the client closed the connection, ignore logging the error
                                //we just assume that they just don't want any more packets
                                error!("Error sending response: {:?}", e);
                            }
                            break;
                        }
                    }
                }
            });
        }
    }
}

async fn receive_gui(
    mut gui_command_receiver: mpsc::UnboundedReceiver<Command>,
    command_sender: CommandSender,
) {
    if cfg!(feature = "gui") {
        while let Some(command) = gui_command_receiver.recv().await {
            let (tx, mut rx) = mpsc::unbounded_channel();
            command_sender.send((command, tx)).unwrap();
            // Ignore all respones for now.
            while let Some(_) = rx.recv().await {} 
        }
    }
}
