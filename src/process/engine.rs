use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::BufReader;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;

use super::reader::{EngineCommandReader, EngineOutput};
use super::writer::GuiCommandWriter;
use crate::error::Error;
use crate::protocol::*;

/// Represents a metadata returned from a USI engine.
#[derive(Clone, Debug, Default)]
pub struct EngineInfo {
    name: String,
    options: HashMap<String, String>,
}

impl EngineInfo {
    /// Returns an engine name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns available engine options.
    pub fn options(&self) -> &HashMap<String, String> {
        &self.options
    }
}

/// `UsiEngineHandler` provides a type-safe interface to the USI engine process.
///
/// # Examples
/// ```no_run
/// use usi::{BestMoveParams, Error, EngineCommand, GuiCommand, UsiEngineHandler};
///
/// let mut handler = UsiEngineHandler::spawn("/path/to/usi_engine", "/path/to/working_dir", &[] as &[&str]).unwrap();
///
/// // Get the USI engine information.
/// let info = handler.get_info().unwrap();
/// assert_eq!("engine name", info.name());
///
/// // Set options and prepare the engine.
/// handler.send_command(&GuiCommand::SetOption("USI_Ponder".to_string(), Some("true".to_string()))).unwrap();
/// handler.prepare().unwrap();
/// handler.send_command(&GuiCommand::UsiNewGame).unwrap();
///
/// // Start listening to the engine output.
/// // You can pass the closure which will be called
/// //   everytime new command is received from the engine.
/// handler.listen(move |output| -> Result<(), Error> {
///     match output.response() {
///         Some(EngineCommand::BestMove(BestMoveParams::MakeMove(
///                      ref best_move_sfen,
///                      ref ponder_move,
///                 ))) => {
///                     assert_eq!("5g5f", best_move_sfen);
///                 }
///         _ => {}
///     }
///     Ok(())
/// }).unwrap();
/// handler.send_command(&GuiCommand::Usi).unwrap();
/// ```
#[derive(Debug)]
pub struct UsiEngineHandler {
    process: Child,
    reader: Option<EngineCommandReader<BufReader<ChildStdout>>>,
    writer: GuiCommandWriter<ChildStdin>,
    handshake_started: bool,
}

impl Drop for UsiEngineHandler {
    fn drop(&mut self) {
        self.kill().unwrap();
    }
}
impl UsiEngineHandler {
    /// Spanws a new process of the specific USI engine.
    pub fn spawn<P, Q, I, S>(engine_path: P, working_dir: Q, args: I) -> Result<Self, Error>
    where
        P: AsRef<OsStr>,
        Q: AsRef<Path>,
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut process = Command::new(engine_path)
            .args(args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let stdin = process.stdin.take().unwrap();
        let stdout = process.stdout.take().unwrap();

        Ok(UsiEngineHandler {
            process,
            reader: Some(EngineCommandReader::new(BufReader::new(stdout))),
            writer: GuiCommandWriter::new(stdin),
            handshake_started: false,
        })
    }

    /// Sends a command to the engine BEFORE the USI handshake.
    ///
    /// This is useful for engines like Fairy-Stockfish that require
    /// configuration before the `usi` command (e.g., `Protocol`, `UCI_Variant`).
    ///
    /// Returns `Error::IllegalOperation` if called after `get_info()`.
    ///
    /// # Examples
    /// ```no_run
    /// use usi::{GuiCommand, UsiEngineHandler};
    ///
    /// let mut handler = UsiEngineHandler::spawn("/path/to/fairy-stockfish", ".", &[] as &[&str]).unwrap();
    ///
    /// // Configure engine BEFORE handshake
    /// handler.send_command_before_handshake(&GuiCommand::SetOption(
    ///     "Protocol".to_string(),
    ///     Some("usi".to_string())
    /// )).unwrap();
    /// handler.send_command_before_handshake(&GuiCommand::SetOption(
    ///     "UCI_Variant".to_string(),
    ///     Some("shogi".to_string())
    /// )).unwrap();
    ///
    /// // Now proceed with normal handshake
    /// let info = handler.get_info().unwrap();
    /// ```
    pub fn send_command_before_handshake(&mut self, command: &GuiCommand) -> Result<(), Error> {
        if self.handshake_started {
            return Err(Error::IllegalOperation);
        }
        self.writer.send(command)
    }

    /// Request metadata such as a name and available options.
    /// Internally `get_info()` sends `usi` command and
    /// records `id` and `option` commands until `usiok` is received.
    /// Returns `Error::IllegalOperation` when called after `listen` method.
    pub fn get_info(&mut self) -> Result<EngineInfo, Error> {
        let reader = match &mut self.reader {
            Some(r) => Ok(r),
            None => Err(Error::IllegalOperation),
        }?;

        self.handshake_started = true;

        let mut info = EngineInfo::default();
        self.writer.send(&GuiCommand::Usi)?;

        loop {
            match reader.next_command() {
                Ok(output) => {
                    match output.response() {
                        Some(EngineCommand::Id(IdParams::Name(name))) => {
                            info.name = name.to_string();
                        }
                        Some(EngineCommand::Option(OptionParams {
                            ref name,
                            ref value,
                        })) => {
                            info.options.insert(
                                name.to_string(),
                                match value {
                                    OptionKind::Check { default: Some(f) } => {
                                        if *f { "true" } else { "false" }.to_string()
                                    }
                                    OptionKind::Spin {
                                        default: Some(n), ..
                                    } => n.to_string(),
                                    OptionKind::Combo {
                                        default: Some(s), ..
                                    } => s.to_string(),
                                    OptionKind::Button { default: Some(s) } => s.to_string(),
                                    OptionKind::String { default: Some(s) } => s.to_string(),
                                    OptionKind::Filename { default: Some(s) } => s.to_string(),
                                    _ => String::new(),
                                },
                            );
                        }
                        Some(EngineCommand::UsiOk) => break,
                        _ => {}
                    }
                }
                Err(Error::IllegalSyntax) => {
                    // Ignore lines that don't parse as valid USI commands
                    // (e.g., UCI-style output from Fairy-Stockfish)
                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        Ok(info)
    }

    /// Prepare the engine to be ready to start a new game.
    /// Internally, `prepare()` sends `isready` command and waits until `readyok` is received.
    /// Returns `Error::IllegalOperation` when called after `listen` method.
    pub fn prepare(&mut self) -> Result<(), Error> {
        let reader = match &mut self.reader {
            Some(r) => Ok(r),
            None => Err(Error::IllegalOperation),
        }?;

        self.writer.send(&GuiCommand::IsReady)?;
        loop {
            match reader.next_command() {
                Ok(output) => {
                    if let Some(EngineCommand::ReadyOk) = output.response() {
                        break;
                    }
                }
                Err(Error::IllegalSyntax) => {
                    // Ignore lines that don't parse as valid USI commands
                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        Ok(())
    }
    /// Sends a command to the engine.
    pub fn send_command(&mut self, command: &GuiCommand) -> Result<(), Error> {
        self.writer.send(command)
    }

    /// Terminates the engine.
    pub fn kill(&mut self) -> Result<(), Error> {
        self.writer.send(&GuiCommand::Quit)?;
        self.process.kill()?;
        Ok(())
    }

    /// Spanws a new thread to monitor outputs from the engine.
    /// `hook` will be called for each USI command received.
    /// `prepare` method can only be called before `listen` method.
    pub fn listen<F, E>(&mut self, mut hook: F) -> Result<(), Error>
    where
        F: FnMut(&EngineOutput) -> Result<(), E> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let mut reader = self.reader.take().ok_or(Error::IllegalOperation)?;

        thread::spawn(move || -> Result<(), Error> {
            loop {
                match reader.next_command() {
                    Ok(output) => {
                        if let Err(e) = hook(&output) {
                            return Err(Error::HandlerError(Box::new(e)));
                        }
                    }
                    Err(Error::IllegalSyntax) => {
                        // Ignore illegal commands.
                        continue;
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
        });

        Ok(())
    }
}
