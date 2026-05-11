use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum AmbiguousWidth {
    Single,
    Double,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Theme {
    Default,
    EyeCare,
}

#[derive(Clone, Parser)]
#[command(name = "agent_terminal")]
pub(crate) struct CliOptions {
    #[arg(long)]
    pub(crate) self_check: bool,
    #[arg(long)]
    pub(crate) show_status_bar: bool,
    #[arg(long, default_value = "Menlo")]
    pub(crate) font_family: String,
    #[arg(long = "font-fallback", value_delimiter = ',')]
    pub(crate) font_fallbacks: Vec<String>,
    #[arg(long = "double-width-char", value_delimiter = ',')]
    pub(crate) double_width_chars: Vec<String>,
    #[arg(long, value_enum, default_value_t = AmbiguousWidth::Single)]
    pub(crate) ambiguous_width: AmbiguousWidth,
    #[arg(long, value_enum, default_value_t = Theme::Default)]
    pub(crate) theme: Theme,
    #[arg(long)]
    pub(crate) input_log_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) input_log_raw: bool,
    #[arg(
        long,
        value_parser = parse_history_log_dir,
        default_value = "~/.rterminal/history",
        help = "Directory for per-tab raw PTY transcript files (.ansi)"
    )]
    pub(crate) history_log_dir: PathBuf,
    #[arg(long, help = "Disable smooth cursor slide animation")]
    pub(crate) no_cursor_slide: bool,
    #[arg(
        long,
        help = "Force a vertical beam cursor regardless of app cursor mode"
    )]
    pub(crate) force_vertical_cursor: bool,
    #[arg(long, help = "Enable subtle trailing effect for vertical beam cursor")]
    pub(crate) cursor_trail: bool,
    #[arg(
        long,
        help = "Treat macOS Option key as plain text input instead of Meta/Alt"
    )]
    pub(crate) no_option_as_meta: bool,
}

#[cfg(test)]
pub(crate) fn parse_cli_options_from<I>(args: I) -> CliOptions
where
    I: IntoIterator<Item = String>,
{
    CliOptions::parse_from(std::iter::once("agent_terminal".to_string()).chain(args))
}

pub(crate) fn parse_cli_options() -> CliOptions {
    CliOptions::parse()
}

fn parse_history_log_dir(value: &str) -> Result<PathBuf, String> {
    if let Some(rest) = value.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            "cannot expand '~' in --history-log-dir because HOME is unset".to_string()
        })?;
        Ok(PathBuf::from(home).join(rest))
    } else if value == "~" {
        std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            "cannot expand '~' in --history-log-dir because HOME is unset".to_string()
        })
    } else {
        Ok(PathBuf::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::{AmbiguousWidth, Theme, parse_cli_options_from};

    #[test]
    fn ambiguous_width_defaults_to_single() {
        let cli = parse_cli_options_from(Vec::<String>::new());
        assert_eq!(cli.ambiguous_width, AmbiguousWidth::Single);
    }

    #[test]
    fn ambiguous_width_accepts_double() {
        let cli =
            parse_cli_options_from(vec!["--ambiguous-width".to_string(), "double".to_string()]);
        assert_eq!(cli.ambiguous_width, AmbiguousWidth::Double);
    }

    #[test]
    fn theme_defaults_to_default() {
        let cli = parse_cli_options_from(Vec::<String>::new());
        assert_eq!(cli.theme, Theme::Default);
    }

    #[test]
    fn theme_accepts_eye_care() {
        let cli = parse_cli_options_from(vec!["--theme".to_string(), "eye-care".to_string()]);
        assert_eq!(cli.theme, Theme::EyeCare);
    }

    #[test]
    fn input_log_file_argument_parses() {
        let cli = parse_cli_options_from(vec![
            "--input-log-file".to_string(),
            "/tmp/agent-input.jsonl".to_string(),
        ]);
        assert_eq!(
            cli.input_log_file.as_deref(),
            Some(std::path::Path::new("/tmp/agent-input.jsonl"))
        );
    }

    #[test]
    fn history_log_dir_defaults_to_rterminal_history() {
        let cli = parse_cli_options_from(Vec::<String>::new());
        assert_eq!(
            cli.history_log_dir,
            std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME set"))
                .join(".rterminal/history")
        );
    }

    #[test]
    fn history_log_dir_argument_parses() {
        let cli = parse_cli_options_from(vec![
            "--history-log-dir".to_string(),
            "/tmp/agent-history".to_string(),
        ]);
        assert_eq!(
            cli.history_log_dir,
            std::path::PathBuf::from("/tmp/agent-history")
        );
    }

    #[test]
    fn cursor_slide_enabled_by_default() {
        let cli = parse_cli_options_from(Vec::<String>::new());
        assert!(!cli.no_cursor_slide);
    }

    #[test]
    fn cursor_slide_can_be_disabled() {
        let cli = parse_cli_options_from(vec!["--no-cursor-slide".to_string()]);
        assert!(cli.no_cursor_slide);
    }

    #[test]
    fn force_vertical_cursor_defaults_to_disabled() {
        let cli = parse_cli_options_from(Vec::<String>::new());
        assert!(!cli.force_vertical_cursor);
    }

    #[test]
    fn force_vertical_cursor_flag_parses() {
        let cli = parse_cli_options_from(vec!["--force-vertical-cursor".to_string()]);
        assert!(cli.force_vertical_cursor);
    }

    #[test]
    fn cursor_trail_defaults_to_disabled() {
        let cli = parse_cli_options_from(Vec::<String>::new());
        assert!(!cli.cursor_trail);
    }

    #[test]
    fn cursor_trail_flag_parses() {
        let cli = parse_cli_options_from(vec!["--cursor-trail".to_string()]);
        assert!(cli.cursor_trail);
    }

    #[test]
    fn font_fallback_accepts_repeated_and_comma_separated_values() {
        let cli = parse_cli_options_from(vec![
            "--font-fallback".to_string(),
            "Symbols Nerd Font Mono,Apple Symbols".to_string(),
            "--font-fallback".to_string(),
            "Noto Sans Symbols".to_string(),
        ]);
        assert_eq!(
            cli.font_fallbacks,
            vec![
                "Symbols Nerd Font Mono".to_string(),
                "Apple Symbols".to_string(),
                "Noto Sans Symbols".to_string()
            ]
        );
    }

    #[test]
    fn double_width_char_accepts_repeated_and_comma_separated_values() {
        let cli = parse_cli_options_from(vec![
            "--double-width-char".to_string(),
            "↑,↓".to_string(),
            "--double-width-char".to_string(),
            "↕".to_string(),
        ]);
        assert_eq!(
            cli.double_width_chars,
            vec!["↑".to_string(), "↓".to_string(), "↕".to_string()]
        );
    }
}
