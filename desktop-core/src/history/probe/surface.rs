//! Which commands are the collector's, for the helper's one command line.

use super::Cli;

/// The collector's commands a person runs, for the helper's `--help`.
pub fn commands() -> Vec<clap::Command> {
    use clap::CommandFactory;
    Cli::command()
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .cloned()
        .collect()
}

/// Whether `arguments` (without the program name) name one of the
/// collector's commands, which [`main`] runs; every other command is the
/// core's.
pub fn handles(arguments: &[String]) -> bool {
    use clap::CommandFactory;
    let Some(name) = arguments.iter().find(|argument| !argument.starts_with('-')) else {
        return false;
    };
    Cli::command()
        .get_subcommands()
        .any(|command| command.get_name() == name)
}
