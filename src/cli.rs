//! Command-line argument parsing.
//!
//! `smartty` takes a command (and its arguments) to run inside the proxied PTY:
//!
//! ```sh
//! smartty zsh
//! smartty -- ssh my-server
//! ```
//!
//! With no command, it falls back to `$SHELL`, then `/bin/sh`.

use std::env;

/// Parsed invocation: the program to run plus its arguments.
pub struct Cli {
    /// The command and its arguments, e.g. `["ssh", "my-server"]`.
    /// Guaranteed non-empty.
    pub command: Vec<String>,
}

impl Cli {
    /// Parse `std::env::args`. A leading `--` separator is accepted and skipped.
    pub fn parse() -> Cli {
        let mut args = env::args().skip(1).peekable();

        // Allow `smartty -- cmd ...` to disambiguate flags from the child command.
        if args.peek().map(|s| s == "--").unwrap_or(false) {
            args.next();
        }

        let command: Vec<String> = args.collect();
        if command.is_empty() {
            return Cli {
                command: vec![default_shell()],
            };
        }
        Cli { command }
    }

    /// The program name (first element of `command`).
    pub fn program(&self) -> &str {
        &self.command[0]
    }

    /// Arguments passed to the program (everything after the program name).
    pub fn args(&self) -> &[String] {
        &self.command[1..]
    }
}

/// Resolve the user's preferred shell, falling back to `/bin/sh`.
fn default_shell() -> String {
    env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}
