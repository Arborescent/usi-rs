# usi-rs

[![Github Actions](https://github.com/nozaq/usi-rs/workflows/build/badge.svg)](https://github.com/nozaq/usi-rs/actions?workflow=build)
[![Coverage Status](https://coveralls.io/repos/github/nozaq/usi-rs/badge.svg)](https://coveralls.io/github/nozaq/usi-rs)
[![crates.io](https://img.shields.io/crates/v/usi.svg)](https://crates.io/crates/usi)
[![docs.rs](https://docs.rs/usi/badge.svg)](https://docs.rs/usi)

A library to handle type-safe communication with USI-compatible shogi engines.
USI protocol defines commands sent from either GUIs or engines. Detail about USI protocol can be found at <http://shogidokoro.starfree.jp/usi.html>.

[Documentation](https://docs.rs/usi)

## Usage

### Data types representing commands defined in USI protocol

GuiCommand and EngineCommand represents input/output commands defined in the protocol.

#### Examples

```rust
use std::time::Duration;
use usi::{GuiCommand, ThinkParams, EngineCommand, BestMoveParams};

// GuiCommand can be converted into the USI compliant string.
let params = ThinkParams::new().btime(Duration::from_secs(1)).wtime(Duration::from_secs(2));
let cmd = GuiCommand::Go(params);
assert_eq!("go btime 1000 wtime 2000", cmd.to_string());

// EngineCommand can be parsed from the command string sent from the USI engine.
let cmd = EngineCommand::parse("bestmove 7g7f ponder 8c8d").unwrap();
match cmd {
    EngineCommand::BestMove(BestMoveParams::MakeMove(ref m, Some(ref pm))) => {
        assert_eq!("7g7f", *m);
        assert_eq!("8c8d", *pm);
    },
    _ => unreachable!(),
}
```

### Working with a USI engine process

UsiEngineHandler can be used to spawn the USI engine process. You can send GuiCommands and receive EngineCommand.

#### Examples

```rust
use usi::{BestMoveParams, Error, EngineCommand, GuiCommand, UsiEngineHandler};

let mut handler = UsiEngineHandler::spawn("/path/to/usi_engine", "/path/to/working_dir").unwrap();

// Get the USI engine information.
let info = handler.get_info().unwrap();
assert_eq!("engine name", info.name());

// Set options.
handler.send_command(&GuiCommand::SetOption("USI_Ponder".to_string(), Some("true".to_string()))).unwrap();
handler.prepare().unwrap();
handler.send_command(&GuiCommand::UsiNewGame).unwrap();

// Start listening to the engine output.
// You can pass the closure which will be called
//   everytime new command is received from the engine.
handler.listen(move |output| -> Result<(), Error> {
    match output.response() {
        Some(EngineCommand::BestMove(BestMoveParams::MakeMove(
                     ref best_move_sfen,
                     ref ponder_move,
                ))) => {
                    assert_eq!("5g5f", best_move_sfen);
                }
        _ => {}
    }
    Ok(())
}).unwrap();
handler.send_command(&GuiCommand::Usi).unwrap();
```

### Pre-Handshake Configuration

Some engines (like Fairy-Stockfish) require configuration before the USI handshake. Use `send_command_before_handshake()` for this:

```rust
use usi::{GuiCommand, UsiEngineHandler};

let mut handler = UsiEngineHandler::spawn("/path/to/fairy-stockfish", ".").unwrap();

// Configure before handshake (required for Fairy-Stockfish)
handler.send_command_before_handshake(&GuiCommand::SetOption(
    "Protocol".to_string(),
    Some("usi".to_string())
)).unwrap();
handler.send_command_before_handshake(&GuiCommand::SetOption(
    "UCI_Variant".to_string(),
    Some("shogi".to_string())
)).unwrap();

// Now proceed with normal handshake
let info = handler.get_info().unwrap();
```

### Threaded Engine Wrapper

For non-blocking engine communication, use `ThreadedEngine`. This spawns the engine in a background thread and provides channel-based messaging:

```rust
use usi::threaded::{ThreadedEngine, EngineConfig};
use std::time::Duration;

let config = EngineConfig {
    path: "/path/to/engine".to_string(),
    working_dir: Some("/path/to/working/dir".to_string()),
    pre_handshake_options: vec![], // For Fairy-Stockfish: vec![("Protocol".into(), Some("usi".into()))]
};

let mut engine = ThreadedEngine::spawn(config).unwrap();

// Configure engine options
engine.set_option("Skill Level", Some("10"));
engine.is_ready();

// Set position and search
engine.set_position("lnsgkgsnl/1r5b1/ppppppppp/9/9/9/PPPPPPPPP/1B5R1/LNSGKGSNL b - 1");
engine.go_byoyomi(Duration::from_secs(5));

// Poll for move (non-blocking)
loop {
    if let Some(mv) = engine.poll_move() {
        println!("Best move: {}", mv);
        break;
    }
    std::thread::sleep(Duration::from_millis(100));
}
```

## Acknowledgements

This library is a fork of [nozaq/usi-rs](https://github.com/nozaq/usi-rs). Thanks to nozaq for the original implementation.

## License

`usi-rs` is licensed under the MIT license. Please read the [LICENSE](LICENSE) file in this repository for more information.
