//! Command-line grammar for `enso-ctl`.

use std::ffi::OsString;

pub const HELP: &str = "\
enso-ctl — control center for the enso desktop

Usage: enso-ctl [COMMAND]

With no command, opens the control center panel.

Commands:
  theme [list]        List available themes
  theme current       Print the active theme's id
  theme set <ID>      Switch theme; running enso tools update instantly
  theme next          Switch to the next theme
  theme show [ID]     Print a theme's palette
  theme dirs          Print where themes are loaded from
  power               Print battery, power profile, temperature, backlight

Planned: wifi, bluetooth, audio.

Options:
  -h, --help          Print this help
  -V, --version       Print the version
";

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the control center panel.
    Panel,
    Help,
    Version,
    Theme(ThemeCommand),
    Power,
    /// A domain that is designed for but not built yet.
    Planned(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeCommand {
    List,
    Current,
    Set(String),
    Next,
    Show(Option<String>),
    Dirs,
}

/// Parse arguments (without the program name).
pub fn parse(args: impl IntoIterator<Item = impl Into<OsString>>) -> Result<Command, String> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    let mut words: Vec<String> = Vec::new();
    while let Some(arg) = parser.next().map_err(|err| err.to_string())? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Short('V') | Long("version") => return Ok(Command::Version),
            Value(value) => words.push(value.string().map_err(|err| err.to_string())?),
            other => return Err(other.unexpected().to_string()),
        }
    }

    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => Ok(Command::Panel),
        ["theme"] | ["theme", "list"] => Ok(Command::Theme(ThemeCommand::List)),
        ["theme", "current"] => Ok(Command::Theme(ThemeCommand::Current)),
        ["theme", "set", id] => Ok(Command::Theme(ThemeCommand::Set((*id).to_owned()))),
        ["theme", "set"] => {
            Err("`theme set` needs a theme id (see `enso-ctl theme list`)".to_owned())
        }
        ["theme", "next"] => Ok(Command::Theme(ThemeCommand::Next)),
        ["theme", "show"] => Ok(Command::Theme(ThemeCommand::Show(None))),
        ["theme", "show", id] => Ok(Command::Theme(ThemeCommand::Show(Some((*id).to_owned())))),
        ["theme", "dirs"] => Ok(Command::Theme(ThemeCommand::Dirs)),
        ["theme", other, ..] => Err(format!("unknown theme command `{other}`")),
        ["power"] => Ok(Command::Power),
        ["wifi", ..] => Ok(Command::Planned("wifi")),
        ["bluetooth", ..] => Ok(Command::Planned("bluetooth")),
        ["audio", ..] => Ok(Command::Planned("audio")),
        [other, ..] => Err(format!("unknown command `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Command, String> {
        parse(words.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_opens_the_panel() {
        assert_eq!(parse_words(&[]), Ok(Command::Panel));
    }

    #[test]
    fn parses_theme_commands() {
        use ThemeCommand::*;
        assert_eq!(parse_words(&["theme"]), Ok(Command::Theme(List)));
        assert_eq!(parse_words(&["theme", "list"]), Ok(Command::Theme(List)));
        assert_eq!(
            parse_words(&["theme", "current"]),
            Ok(Command::Theme(Current))
        );
        assert_eq!(parse_words(&["theme", "next"]), Ok(Command::Theme(Next)));
        assert_eq!(
            parse_words(&["theme", "set", "rose-pine-dawn"]),
            Ok(Command::Theme(Set("rose-pine-dawn".into())))
        );
        assert_eq!(
            parse_words(&["theme", "show"]),
            Ok(Command::Theme(Show(None)))
        );
        assert_eq!(
            parse_words(&["theme", "show", "x"]),
            Ok(Command::Theme(Show(Some("x".into()))))
        );
    }

    #[test]
    fn flags_and_errors() {
        assert_eq!(parse_words(&["--help"]), Ok(Command::Help));
        assert_eq!(parse_words(&["theme", "-h"]), Ok(Command::Help));
        assert_eq!(parse_words(&["-V"]), Ok(Command::Version));
        assert_eq!(parse_words(&["power"]), Ok(Command::Power));
        assert_eq!(parse_words(&["wifi", "scan"]), Ok(Command::Planned("wifi")));
        assert!(parse_words(&["theme", "set"]).is_err());
        assert!(parse_words(&["theme", "paint"]).is_err());
        assert!(parse_words(&["nope"]).is_err());
        assert!(parse_words(&["--nope"]).is_err());
    }
}
