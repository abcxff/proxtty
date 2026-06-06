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

/// Parsed invocation: the program to run plus its arguments (may be empty, in
/// which case [`Cli::resolve`] fills in a default).
pub struct Cli {
    command: Vec<String>,
}

impl Cli {
    /// Parse `std::env::args`. A leading `--` separator is accepted and skipped.
    pub fn parse() -> Cli {
        let mut args = env::args().skip(1).peekable();

        // Allow `smartty -- cmd ...` to disambiguate flags from the child command.
        if args.peek().map(|s| s == "--").unwrap_or(false) {
            args.next();
        }

        Cli {
            command: args.collect(),
        }
    }

    /// The command to run: the one given on the command line, or — when none was
    /// given — the configured shell, then `$SHELL`, then `/bin/sh`. Always
    /// non-empty.
    pub fn resolve(self, config_shell: Option<&str>) -> Vec<String> {
        if !self.command.is_empty() {
            return self.command;
        }
        let shell = config_shell
            .map(str::to_string)
            .unwrap_or_else(default_shell);
        vec![shell]
    }
}

/// Resolve the user's preferred shell, falling back to `/bin/sh`.
fn default_shell() -> String {
    env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}
