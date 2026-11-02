//! Async tmux control-mode client over any byte stream.
//!
//! A single reader task owns the protocol parser. Commands are written as
//! lines; tmux answers every command from this client with a `%begin` block
//! flagged `1`, strictly in order, so replies are matched FIFO.
//!
//! Replies can be routed two ways:
//! * [`ControlClient::call`] — awaited through a oneshot (for actions whose
//!   only interesting outcome is success/failure);
//! * [`ControlClient::call_tagged`] — delivered *in stream order* through the
//!   event channel as [`ControlEvent::Reply`]. This matters for state that
//!   races with notifications: a `capture-pane` reply reflects exactly the
//!   `%output` received before it and none after, and a `list-windows` reply
//!   must not be overwritten by older `%layout-change` lines still queued.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tmux_control::protocol::{Event, Notification, Parser, Response};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, thiserror::Error, Clone)]
pub enum ControlError {
    #[error("tmux: {0}")]
    Command(String),
    #[error("control connection closed")]
    Closed,
}

#[derive(Debug)]
pub enum ControlEvent {
    Notification(Notification),
    Reply { tag: u64, response: Response },
    /// Non-protocol text outside blocks (e.g. shell noise before tmux
    /// started, or an error message from a failed `tmux` invocation).
    Stray(String),
    /// The stream ended. `exit` holds the `%exit` reason if tmux said
    /// goodbye cleanly; `None` means the transport dropped.
    Closed { exit: Option<Option<String>> },
}

enum Pending {
    Oneshot(oneshot::Sender<Result<Response, ControlError>>),
    Tagged(u64),
}

struct Shared {
    pending: VecDeque<Pending>,
    closed: bool,
}

#[derive(Clone)]
pub struct ControlClient {
    shared: Arc<Mutex<Shared>>,
    writer: mpsc::UnboundedSender<Vec<u8>>,
}

impl ControlClient {
    /// Start the client. Returns the handle and the ordered event stream.
    pub fn start<S>(stream: S) -> (ControlClient, mpsc::UnboundedReceiver<ControlEvent>)
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (mut rd, mut wr) = tokio::io::split(stream);
        let shared = Arc::new(Mutex::new(Shared { pending: VecDeque::new(), closed: false }));
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let (w_tx, mut w_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        tokio::spawn(async move {
            while let Some(buf) = w_rx.recv().await {
                if wr.write_all(&buf).await.is_err() || wr.flush().await.is_err() {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        });

        let sh = shared.clone();
        tokio::spawn(async move {
            let mut parser = Parser::new();
            let mut buf = vec![0u8; 64 * 1024];
            let mut events = Vec::new();
            let mut exit: Option<Option<String>> = None;
            loop {
                let n = match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                parser.feed(&buf[..n], &mut events);
                for ev in events.drain(..) {
                    match ev {
                        Event::Response(r) => {
                            if !r.from_this_client() {
                                continue; // the initial attach / config commands
                            }
                            let p = sh.lock().unwrap().pending.pop_front();
                            match p {
                                Some(Pending::Oneshot(tx)) => {
                                    let _ = tx.send(if r.success {
                                        Ok(r)
                                    } else {
                                        Err(ControlError::Command(r.text()))
                                    });
                                }
                                Some(Pending::Tagged(tag)) => {
                                    let _ = ev_tx.send(ControlEvent::Reply { tag, response: r });
                                }
                                None => tracing::warn!("unsolicited reply #{}", r.cmdnum),
                            }
                        }
                        Event::Notification(Notification::Exit { reason }) => {
                            exit = Some(reason.clone());
                            let _ = ev_tx.send(ControlEvent::Notification(Notification::Exit { reason }));
                        }
                        Event::Notification(n) => {
                            let _ = ev_tx.send(ControlEvent::Notification(n));
                        }
                        Event::Stray(l) => {
                            let _ = ev_tx.send(ControlEvent::Stray(String::from_utf8_lossy(&l).into_owned()));
                        }
                    }
                }
            }
            let pending = {
                let mut s = sh.lock().unwrap();
                s.closed = true;
                std::mem::take(&mut s.pending)
            };
            for p in pending {
                if let Pending::Oneshot(tx) = p {
                    let _ = tx.send(Err(ControlError::Closed));
                }
            }
            let _ = ev_tx.send(ControlEvent::Closed { exit });
        });

        (ControlClient { shared, writer: w_tx }, ev_rx)
    }

    fn enqueue(&self, cmd: &str, p: Pending) -> Result<(), ControlError> {
        debug_assert!(!cmd.contains('\n'), "control commands are single lines");
        // Hold the lock across send so queue order == write order.
        let mut s = self.shared.lock().unwrap();
        if s.closed {
            return Err(ControlError::Closed);
        }
        let mut line = cmd.as_bytes().to_vec();
        line.push(b'\n');
        self.writer.send(line).map_err(|_| ControlError::Closed)?;
        s.pending.push_back(p);
        Ok(())
    }

    /// Run a command and wait for its reply.
    pub async fn call(&self, cmd: &str) -> Result<Response, ControlError> {
        let (tx, rx) = oneshot::channel();
        self.enqueue(cmd, Pending::Oneshot(tx))?;
        rx.await.map_err(|_| ControlError::Closed)?
    }

    /// Run a command whose reply arrives in-order on the event stream.
    pub fn call_tagged(&self, cmd: &str, tag: u64) -> Result<(), ControlError> {
        self.enqueue(cmd, Pending::Tagged(tag))
    }

    /// Send several tagged commands in a single write.
    ///
    /// tmux queues every command line from one read and runs the whole queue
    /// before servicing pane output again (server_loop drains cmdq before
    /// returning to the event loop), so a batch observes one consistent
    /// instant: e.g. pane state + capture with no output in between.
    pub fn call_tagged_batch(&self, cmds: &[(String, u64)]) -> Result<(), ControlError> {
        let mut s = self.shared.lock().unwrap();
        if s.closed {
            return Err(ControlError::Closed);
        }
        let mut buf = Vec::new();
        for (cmd, _) in cmds {
            debug_assert!(!cmd.contains('\n'));
            buf.extend_from_slice(cmd.as_bytes());
            buf.push(b'\n');
        }
        self.writer.send(buf).map_err(|_| ControlError::Closed)?;
        for (_, tag) in cmds {
            s.pending.push_back(Pending::Tagged(*tag));
        }
        Ok(())
    }

    /// Fire and forget; errors are logged.
    pub fn send(&self, cmd: &str) {
        let c = self.clone();
        let cmd = cmd.to_string();
        tokio::spawn(async move {
            if let Err(e) = c.call(&cmd).await {
                tracing::debug!("{cmd}: {e}");
            }
        });
    }

    /// Detach this client (tmux keeps the session running). An empty line
    /// is the control-mode detach request.
    pub fn detach(&self) {
        let _ = self.writer.send(b"\n".to_vec());
    }

    pub fn is_closed(&self) -> bool {
        self.shared.lock().unwrap().closed
    }
}
