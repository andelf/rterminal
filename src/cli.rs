#[derive(Clone)]
pub(crate) struct CliOptions {
    pub(crate) self_check: bool,
    pub(crate) show_status_bar: bool,
    pub(crate) font_family: String,
}

impl Default for CliOptions {
    fn default() -> Self {
        Self {
            self_check: false,
            show_status_bar: false,
            font_family: "Menlo".to_string(),
        }
    }
}

pub(crate) fn parse_cli_options_from<I>(args: I) -> CliOptions
where
    I: IntoIterator<Item = String>,
{
    let mut options = CliOptions::default();
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--self-check" => options.self_check = true,
            "--show-status-bar" => options.show_status_bar = true,
            "--font-family" => {
                if let Some(value) = iter.next() {
                    let value = value.trim();
                    if !value.is_empty() {
                        options.font_family = value.to_string();
                    }
                }
            }
            _ if arg.starts_with("--font-family=") => {
                let value = arg.trim_start_matches("--font-family=").trim();
                if !value.is_empty() {
                    options.font_family = value.to_string();
                }
            }
            _ => {}
        }
    }
    options
}

pub(crate) fn parse_cli_options() -> CliOptions {
    parse_cli_options_from(std::env::args().skip(1))
}
