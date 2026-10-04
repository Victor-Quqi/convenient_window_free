//! Exactly one native listener launches commands for each managed helper.
use serde_json::Value;
use std::net::{Shutdown, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;
use tungstenite::{Message, WebSocket};

pub struct CommandBroker {
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<TcpStream>>,
    thread: Option<JoinHandle<()>>,
}

impl CommandBroker {
    pub fn start(socket: WebSocket<TcpStream>, token: String, pid: u32) -> Result<Self, String> {
        Self::start_with(
            socket,
            move || crate::supervisor::ready_socket(&token, pid),
            launch_command,
        )
    }

    fn start_with(
        mut socket: WebSocket<TcpStream>,
        mut reconnect: impl FnMut() -> Result<WebSocket<TcpStream>, String> + Send + 'static,
        mut launch: impl FnMut(&str) -> Result<(), String> + Send + 'static,
    ) -> Result<Self, String> {
        configure_socket(&socket)?;
        let stream = socket
            .get_ref()
            .try_clone()
            .map_err(|error| error.to_string())?;
        let stream = Arc::new(Mutex::new(stream));
        let current_stream = Arc::clone(&stream);
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("desktop-command-broker".into())
            .spawn(move || {
                while !cancelled.load(Ordering::Acquire) {
                    let disconnected = match socket.read() {
                        Ok(Message::Text(text)) => {
                            if cancelled.load(Ordering::Acquire) {
                                break;
                            }
                            if let Some(command) = command_from_message(&text) {
                                if let Err(error) = launch(&command) {
                                    eprintln!("Command launch failed: {error}");
                                }
                            }
                            false
                        }
                        Ok(Message::Close(_)) => true,
                        Ok(_) => false,
                        Err(tungstenite::Error::Io(error))
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            false
                        }
                        Err(_) => true,
                    };
                    if disconnected {
                        loop {
                            if cancelled.load(Ordering::Acquire) {
                                return;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(200));
                            if cancelled.load(Ordering::Acquire) {
                                return;
                            }
                            let Ok(connection) = reconnect() else {
                                continue;
                            };
                            if configure_socket(&connection).is_err() {
                                continue;
                            }
                            let Ok(replacement) = connection.get_ref().try_clone() else {
                                continue;
                            };
                            let mut current = current_stream
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            // Publish and cancel under the same lock: Drop must always
                            // close the active connection, even while reconnection wins a race.
                            if cancelled.load(Ordering::Acquire) {
                                let _ = replacement.shutdown(Shutdown::Both);
                                return;
                            }
                            *current = replacement;
                            socket = connection;
                            break;
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stop,
            stream,
            thread: Some(thread),
        })
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for CommandBroker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let stream = self
            .stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = stream.shutdown(Shutdown::Both);
        drop(stream);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn configure_socket(socket: &WebSocket<TcpStream>) -> Result<(), String> {
    // Windows shutdown of a cloned socket need not wake a pending read immediately.
    // Bound worker cancellation on both the first and every replacement connection.
    let timeout = Some(std::time::Duration::from_millis(200));
    socket
        .get_ref()
        .set_read_timeout(timeout)
        .map_err(|error| error.to_string())?;
    socket
        .get_ref()
        .set_write_timeout(timeout)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn command_from_message(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("type")?.as_str()? != "desktop.command" {
        return None;
    }
    let command = value.get("data")?.get("command")?.as_str()?;
    (!command.trim().is_empty() && !command.contains('\0')).then(|| command.to_owned())
}

fn launch_command(command: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        if crate::windows_process::current_elevated()? {
            return Err("Desktop must run with ordinary permissions".into());
        }
        use std::os::windows::process::CommandExt;
        let executable = std::env::var_os("SystemRoot").ok_or("SystemRoot is unavailable")?;
        std::process::Command::new(
            std::path::PathBuf::from(executable)
                .join("System32")
                .join("cmd.exe"),
        )
        .args(["/D", "/S", "/C"])
        .raw_arg(format!("\"{command}\""))
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|error| error.to_string())?;
    }
    #[cfg(not(windows))]
    std::process::Command::new("sh")
        .args(["-c", command])
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket_pair() -> (WebSocket<TcpStream>, WebSocket<TcpStream>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            tungstenite::accept(stream).unwrap()
        });
        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let (socket, _) = tungstenite::client(format!("ws://{address}"), stream).unwrap();
        (socket, peer.join().unwrap())
    }

    #[test]
    fn close_frame_reconnects_and_dispatches_commands_once() {
        let (socket, mut first_peer) = socket_pair();
        let (replacement, mut next_peer) = socket_pair();
        let mut replacement = Some(replacement);
        let (sent, received) = std::sync::mpsc::channel();
        let broker = CommandBroker::start_with(
            socket,
            move || {
                replacement
                    .take()
                    .ok_or_else(|| "no more connections".into())
            },
            move |command| {
                sent.send(command.to_string())
                    .map_err(|error| error.to_string())
            },
        )
        .unwrap();
        first_peer.close(None).unwrap();
        next_peer.send(Message::Text(
            serde_json::json!({"type":"desktop.command", "data":{"command":"echo after reconnect"}}).to_string().into(),
        )).unwrap();
        let result = received.recv_timeout(std::time::Duration::from_secs(2));
        drop(broker);
        assert_eq!(result.unwrap(), "echo after reconnect");
        assert!(received.try_recv().is_err(), "command must not be replayed");
    }

    #[test]
    fn drop_shuts_down_the_reconnected_socket_not_the_old_one() {
        let (socket, first_peer) = socket_pair();
        let (replacement, mut next_peer) = socket_pair();
        let mut replacement = Some(replacement);
        let broker = CommandBroker::start_with(
            socket,
            move || {
                replacement
                    .take()
                    .ok_or_else(|| "no more connections".into())
            },
            |_| Ok(()),
        )
        .unwrap();
        first_peer.get_ref().shutdown(Shutdown::Both).unwrap();
        // A pong proves the new connection has entered its next blocking read.
        next_peer.send(Message::Ping(vec![1, 2, 3].into())).unwrap();
        let pong = next_peer.read().unwrap();
        assert!(matches!(pong, Message::Pong(_)));
        assert_eq!(
            broker.stream.lock().unwrap().local_addr().unwrap(),
            next_peer.get_ref().peer_addr().unwrap()
        );
        let started = std::time::Instant::now();
        drop(broker);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "Drop waited for a timeout on the replacement connection"
        );
        assert!(
            next_peer.read().is_err(),
            "the replacement socket must be closed"
        );
    }

    #[test]
    fn reconnect_retries_transient_failures_before_dispatching() {
        let (socket, mut first_peer) = socket_pair();
        let (replacement, mut next_peer) = socket_pair();
        let mut replacement = Some(replacement);
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reconnect_attempts = Arc::clone(&attempts);
        let (sent, received) = std::sync::mpsc::channel();
        let broker = CommandBroker::start_with(
            socket,
            move || {
                if reconnect_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("helper is not ready yet".into());
                }
                replacement
                    .take()
                    .ok_or_else(|| "no more connections".into())
            },
            move |command| {
                sent.send(command.to_string())
                    .map_err(|error| error.to_string())
            },
        )
        .unwrap();
        first_peer.close(None).unwrap();
        next_peer
            .send(Message::Text(
                serde_json::json!({"type":"desktop.command", "data":{"command":"after retry"}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        let result = received.recv_timeout(std::time::Duration::from_secs(2));
        drop(broker);
        assert_eq!(result.unwrap(), "after retry");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn drop_during_reconnect_closes_the_late_connection_without_launching() {
        let (socket, first_peer) = socket_pair();
        let (replacement, mut next_peer) = socket_pair();
        let mut replacement = Some(replacement);
        let (connecting, connected) = std::sync::mpsc::channel();
        let (resume, resumed) = std::sync::mpsc::channel();
        let broker = CommandBroker::start_with(
            socket,
            move || {
                connecting.send(()).map_err(|error| error.to_string())?;
                resumed
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|error| error.to_string())?;
                replacement
                    .take()
                    .ok_or_else(|| "no more connections".into())
            },
            |_| panic!("a cancelled broker must never launch a command"),
        )
        .unwrap();
        first_peer.get_ref().shutdown(Shutdown::Both).unwrap();
        connected
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let stopped = Arc::clone(&broker.stop);
        let dropper = std::thread::spawn(move || drop(broker));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !stopped.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        let cancelled = stopped.load(Ordering::Acquire);
        resume.send(()).unwrap();
        dropper.join().unwrap();
        assert!(
            cancelled,
            "Drop must cancel before publishing a replacement socket"
        );
        assert!(
            next_peer.read().is_err(),
            "late connection must not outlive the broker"
        );
    }

    #[cfg(windows)]
    #[test]
    fn native_listener_runs_commands_with_ordinary_permissions() {
        use std::net::TcpListener;
        use std::time::{Duration, Instant};
        if crate::windows_process::current_elevated().unwrap() {
            assert!(launch_command("exit 0").is_err());
            return;
        }
        let output = std::env::temp_dir().join(format!("cw-command-{}.txt", uuid::Uuid::new_v4()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let destination = output.to_string_lossy().replace('\'', "''");
        let command = format!("powershell.exe -NoProfile -NonInteractive -Command \"[IO.File]::WriteAllText('{destination}', ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator).ToString())\"");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({"type":"desktop.command", "data":{"command":command}})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            let _ = socket.read();
        });
        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let (socket, _) = tungstenite::client(format!("ws://{address}"), stream).unwrap();
        let broker =
            CommandBroker::start(socket, "unused-test-token".into(), std::process::id()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut result = String::new();
        while Instant::now() < deadline {
            result = std::fs::read_to_string(&output).unwrap_or_default();
            if !result.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(broker);
        server.join().unwrap();
        let _ = std::fs::remove_file(output);
        assert_eq!(result, "False");
    }
    #[test]
    fn only_explicit_command_events_launch_programs() {
        assert_eq!(
            command_from_message(r#"{"type":"desktop.command","data":{"command":"echo hello"}}"#)
                .as_deref(),
            Some("echo hello")
        );
        for message in [
            r#"{"type":"action.triggered","data":{"command":"echo hello"}}"#,
            r#"{"type":"desktop.command","data":{"command":" "}}"#,
            r#"{"type":"desktop.command","data":{"command":"echo\u0000blocked"}}"#,
            r#"{"type":"desktop.command","data":{"command":42}}"#,
            "invalid",
        ] {
            assert!(command_from_message(message).is_none());
        }
    }
}
