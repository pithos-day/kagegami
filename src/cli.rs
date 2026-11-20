use std::path::PathBuf;

pub const USAGE: &str = "usage: kagegami [-h|--help] [--fps N] <output-name|*> <wallpaper.lua>";

#[derive(Debug, Clone, PartialEq)]
pub enum OutputSelector {
    All,
    Name(String),
}

impl OutputSelector {
    pub fn matches(&self, name: &str) -> bool {
        match self {
            OutputSelector::All => true,
            OutputSelector::Name(n) => n == name,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    pub fps: f64,
    pub output: OutputSelector,
    pub script: PathBuf,
}

#[derive(Debug, PartialEq)]
pub enum ParseResult {
    Run(Args),
    Help,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<ParseResult, String> {
    let mut fps = 30.0;
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParseResult::Help),
            "--fps" => {
                let v = iter.next().ok_or("--fps requires a value")?;
                fps = v
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .ok_or_else(|| format!("invalid --fps value: {v}"))?;
            }
            s if s.starts_with("--") => return Err(format!("unknown option: {s}")),
            _ => positional.push(arg),
        }
    }
    let [output, script]: [String; 2] = positional
        .try_into()
        .map_err(|_| "expected <output-name|*> and <wallpaper.lua>".to_string())?;
    let output = if output == "*" {
        OutputSelector::All
    } else {
        OutputSelector::Name(output)
    };
    Ok(ParseResult::Run(Args {
        fps,
        output,
        script: PathBuf::from(script),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<ParseResult, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_full() {
        assert_eq!(
            p(&["--fps", "24", "DP-1", "a.lua"]),
            Ok(ParseResult::Run(Args {
                fps: 24.0,
                output: OutputSelector::Name("DP-1".into()),
                script: "a.lua".into()
            }))
        );
    }

    #[test]
    fn star_means_all_and_default_fps() {
        let Ok(ParseResult::Run(a)) = p(&["*", "a.lua"]) else { panic!() };
        assert_eq!(a.output, OutputSelector::All);
        assert_eq!(a.fps, 30.0);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&["--fps", "0", "*", "a.lua"]).is_err());
        assert!(p(&["--fps"]).is_err());
        assert!(p(&["*"]).is_err());
        assert!(p(&["--bogus", "*", "a.lua"]).is_err());
        assert_eq!(p(&["-h"]), Ok(ParseResult::Help));
    }
}
