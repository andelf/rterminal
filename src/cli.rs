use clap::{Parser, ValueEnum};

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
    #[arg(long, value_enum, default_value_t = AmbiguousWidth::Single)]
    pub(crate) ambiguous_width: AmbiguousWidth,
    #[arg(long, value_enum, default_value_t = Theme::Default)]
    pub(crate) theme: Theme,
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
}
