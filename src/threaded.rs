//! Threaded USI engine wrapper
//!
//! Provides a non-blocking interface to USI engines by running the engine
//! communication in a background thread with channel-based messaging.
//!
//! # Example
//!
//! ```no_run
//! use usi::threaded::{ThreadedEngine, EngineConfig};
//! use std::time::Duration;
//!
//! let config = EngineConfig {
//!     path: "/path/to/engine".to_string(),
//!     working_dir: Some("/path/to/working/dir".to_string()),
//!     pre_handshake_options: vec![],
//! };
//!
//! let mut engine = ThreadedEngine::spawn(config).unwrap();
//!
//! // Set position
//! engine.set_position("lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1");
//!
//! // Start search with byoyomi
//! engine.go_byoyomi(Duration::from_secs(5));
//!
//! // Poll for move (non-blocking)
//! loop {
//!     if let Some(mv) = engine.poll_move() {
//!         println!("Best move: {}", mv);
//!         break;
//!     }
//!     std::thread::sleep(Duration::from_millis(100));
//! }
//! ```

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::error::Error;
use crate::protocol::*;
use crate::process::UsiEngineHandler;

/// Configuration for spawning a threaded USI engine
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Path to the engine executable
    pub path: String,
    /// Working directory for the engine (defaults to engine's parent directory)
    pub working_dir: Option<String>,
    /// Options to send before the USI handshake (for engines like Fairy-Stockfish)
    pub pre_handshake_options: Vec<(String, Option<String>)>,
}

/// Commands sent to the engine thread
#[derive(Debug)]
enum ThreadCommand {
    SetOption { name: String, value: Option<String> },
    IsReady,
    Position { sfen: String },
    Go(ThinkParams),
    Stop,
    Quit,
}

/// A threaded wrapper around `UsiEngineHandler` that provides non-blocking access.
///
/// This spawns the engine in a background thread and uses channels for communication,
/// allowing the caller to send commands and poll for moves without blocking.
pub struct ThreadedEngine {
    command_sender: Sender<ThreadCommand>,
    move_receiver: Arc<Mutex<Receiver<String>>>,
    engine_name: String,
}

impl ThreadedEngine {
    /// Spawn a new threaded USI engine.
    ///
    /// This spawns the engine process and performs the USI handshake in a background thread.
    /// Returns immediately with a handle for sending commands and receiving moves.
    pub fn spawn(config: EngineConfig) -> Result<Self, Error> {
        let path = PathBuf::from(&config.path);
        let work_dir = config
            .working_dir
            .map(PathBuf::from)
            .or_else(|| path.parent().map(|p| p.to_path_buf()))
            .ok_or_else(|| Error::EngineIo(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine engine working directory",
            )))?;

        // Create channels for communication
        let (command_sender, command_receiver) = channel::<ThreadCommand>();
        let (move_sender, move_receiver) = channel::<String>();
        let (name_sender, name_receiver) = channel::<String>();
        let move_receiver = Arc::new(Mutex::new(move_receiver));

        let engine_path = config.path.clone();
        let pre_handshake_options = config.pre_handshake_options.clone();

        thread::spawn(move || {
            Self::engine_thread(
                engine_path,
                work_dir,
                pre_handshake_options,
                command_receiver,
                move_sender,
                name_sender,
            );
        });

        // Wait for engine name (with timeout)
        let engine_name = name_receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| "Unknown Engine".to_string());

        Ok(Self {
            command_sender,
            move_receiver,
            engine_name,
        })
    }

    /// Returns the engine name reported during the USI handshake.
    pub fn name(&self) -> &str {
        &self.engine_name
    }

    /// Set the current position using SFEN notation.
    pub fn set_position(&mut self, sfen: &str) {
        let _ = self.command_sender.send(ThreadCommand::Position {
            sfen: sfen.to_string(),
        });
    }

    /// Start a search with the given parameters.
    pub fn go(&mut self, params: ThinkParams) {
        let _ = self.command_sender.send(ThreadCommand::Go(params));
    }

    /// Start a search with byoyomi time control.
    pub fn go_byoyomi(&mut self, time: Duration) {
        self.go(ThinkParams::new().byoyomi(time));
    }

    /// Start an infinite search.
    pub fn go_infinite(&mut self) {
        self.go(ThinkParams::new().infinite());
    }

    /// Start a mate search.
    pub fn go_mate(&mut self, timeout: Option<Duration>) {
        let params = match timeout {
            Some(t) => ThinkParams::new().mate(MateParam::Timeout(t)),
            None => ThinkParams::new().mate(MateParam::Infinite),
        };
        self.go(params);
    }

    /// Poll for a move result (non-blocking).
    ///
    /// Returns `Some(move_string)` if the engine has produced a move,
    /// `None` if still thinking or no move available.
    pub fn poll_move(&mut self) -> Option<String> {
        if let Ok(receiver) = self.move_receiver.lock() {
            match receiver.try_recv() {
                Ok(mv) => Some(mv),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => None,
            }
        } else {
            None
        }
    }

    /// Stop the current search.
    pub fn stop(&mut self) {
        let _ = self.command_sender.send(ThreadCommand::Stop);
    }

    /// Set an engine option.
    ///
    /// Sends a `setoption` command to the engine. Call `is_ready()` after
    /// setting options to ensure the engine has processed them.
    pub fn set_option(&mut self, name: &str, value: Option<&str>) {
        let _ = self.command_sender.send(ThreadCommand::SetOption {
            name: name.to_string(),
            value: value.map(|v| v.to_string()),
        });
    }

    /// Wait for the engine to be ready.
    ///
    /// Sends an `isready` command to ensure the engine has processed
    /// all previous commands.
    pub fn is_ready(&mut self) {
        let _ = self.command_sender.send(ThreadCommand::IsReady);
    }

    /// Engine thread that manages the USI engine process
    fn engine_thread(
        engine_path: String,
        work_dir: PathBuf,
        pre_handshake_options: Vec<(String, Option<String>)>,
        command_receiver: Receiver<ThreadCommand>,
        move_sender: Sender<String>,
        name_sender: Sender<String>,
    ) {
        // Spawn the engine process
        let mut handler = match UsiEngineHandler::spawn(&engine_path, &work_dir) {
            Ok(h) => h,
            Err(_) => {
                let _ = name_sender.send("Engine Failed".to_string());
                return;
            }
        };

        // Send pre-handshake options (for Fairy-Stockfish, etc.)
        for (name, value) in pre_handshake_options {
            let _ = handler.send_command_before_handshake(&GuiCommand::SetOption(name, value));
        }

        // Get engine info (initiates handshake)
        let engine_name = if let Ok(info) = handler.get_info() {
            info.name().to_string()
        } else {
            "Unknown".to_string()
        };
        let _ = name_sender.send(engine_name);

        // Prepare engine
        if handler.prepare().is_err() {
            return;
        }

        // Send usinewgame
        if handler.send_command(&GuiCommand::UsiNewGame).is_err() {
            return;
        }

        // Start listening to engine output
        let output_sender = move_sender.clone();
        if handler
            .listen(move |output| -> Result<(), std::io::Error> {
                match output.response() {
                    Some(EngineCommand::BestMove(params)) => {
                        match params {
                            BestMoveParams::MakeMove(mv, _ponder) => {
                                let _ = output_sender.send(mv.clone());
                            }
                            BestMoveParams::Resign => {
                                let _ = output_sender.send("resign".to_string());
                            }
                            BestMoveParams::Win => {
                                // Engine claims win, no move to send
                            }
                        }
                    }
                    Some(EngineCommand::Checkmate(params)) => {
                        use crate::protocol::CheckmateParams;
                        match params {
                            CheckmateParams::Mate(moves) => {
                                if let Some(first_move) = moves.first() {
                                    let _ = output_sender.send(first_move.clone());
                                }
                            }
                            CheckmateParams::NoMate
                            | CheckmateParams::NotImplemented
                            | CheckmateParams::Timeout => {
                                let _ = output_sender.send("resign".to_string());
                            }
                        }
                    }
                    _ => {}
                }
                Ok(())
            })
            .is_err()
        {
            return;
        }

        // Process commands from the caller
        while let Ok(cmd) = command_receiver.recv() {
            match cmd {
                ThreadCommand::SetOption { name, value } => {
                    let _ = handler.send_command(&GuiCommand::SetOption(name, value));
                }
                ThreadCommand::IsReady => {
                    let _ = handler.send_command(&GuiCommand::IsReady);
                }
                ThreadCommand::Position { sfen } => {
                    let _ = handler.send_command(&GuiCommand::Position(sfen));
                }
                ThreadCommand::Go(params) => {
                    let _ = handler.send_command(&GuiCommand::Go(params));
                }
                ThreadCommand::Stop => {
                    let _ = handler.send_command(&GuiCommand::Stop);
                }
                ThreadCommand::Quit => {
                    let _ = handler.send_command(&GuiCommand::Quit);
                    break;
                }
            }
        }
    }
}

impl Drop for ThreadedEngine {
    fn drop(&mut self) {
        let _ = self.command_sender.send(ThreadCommand::Quit);
    }
}