use std::path::PathBuf;

pub const USAGE: &str =
    "usage: kagegami [-o OUTPUT] [-f FPS] [-l background|bottom|top|overlay] <wallpaper.lua>";

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LayerName {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    pub fps: f64,
    pub layer: LayerName,
    pub output: OutputSelector,
    pub script: PathBuf,
}

#[derive(Debug, PartialEq)]
pub enum ParseResult {
    Run(Args),
    Help,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<ParseResult, String> {
    let mut fps = 60.0;
    let mut layer = LayerName::Background;
    let mut output = OutputSelector::All;
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(ParseResult::Help),
            "-o" | "--output" => {
                let v = iter.next().ok_or(format!("{arg} requires a value"))?;
                output = OutputSelector::Name(v);
            }
            "-f" | "--fps" => {
                let v = iter.next().ok_or(format!("{arg} requires a value"))?;
                fps = v
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .ok_or_else(|| format!("invalid {arg} value: {v}"))?;
            }
            "-l" | "--layer" => {
                let v = iter.next().ok_or(format!("{arg} requires a value"))?;
                layer = match v.as_str() {
                    "background" => LayerName::Background,
                    "bottom" => LayerName::Bottom,
                    "top" => LayerName::Top,
                    "overlay" => LayerName::Overlay,
                    _ => return Err(format!("invalid {arg} value: {v}")),
                };
            }
            s if s.starts_with('-') && s != "-" => return Err(format!("unknown option: {s}")),
            _ => positional.push(arg),
        }
    }
    let script = match <[String; 1]>::try_from(positional) {
        Ok([script]) => script,
        Err(p) if p.is_empty() => return Err("missing <wallpaper.lua>".into()),
        Err(_) => {
            return Err(
                "expected only <wallpaper.lua> (choose an output with -o NAME; all outputs by default)"
                    .into(),
            );
        }
    };
    Ok(ParseResult::Run(Args {
        fps,
        layer,
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

    fn run(args: &[&str]) -> Args {
        match p(args) {
            Ok(ParseResult::Run(a)) => a,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn defaults_to_all_outputs() {
        assert_eq!(
            run(&["a.lua"]),
            Args {
                fps: 60.0,
                layer: LayerName::Background,
                output: OutputSelector::All,
                script: "a.lua".into()
            }
        );
    }

    #[test]
    fn parses_options() {
        let a = run(&["-o", "DP-1", "--fps", "24", "--layer", "overlay", "a.lua"]);
        assert_eq!(a.output, OutputSelector::Name("DP-1".into()));
        assert_eq!(a.fps, 24.0);
        assert_eq!(a.layer, LayerName::Overlay);
        let a = run(&["a.lua", "--output", "HDMI-A-1"]);
        assert_eq!(a.output, OutputSelector::Name("HDMI-A-1".into()));
        let a = run(&["-f", "120", "-l", "top", "a.lua"]);
        assert_eq!(a.fps, 120.0);
        assert_eq!(a.layer, LayerName::Top);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&[]).is_err());
        assert!(p(&["--fps", "0", "a.lua"]).is_err());
        assert!(p(&["--fps"]).is_err());
        assert!(p(&["-o"]).is_err());
        assert!(p(&["-f"]).is_err());
        assert!(p(&["-l", "middle", "a.lua"]).is_err());
        assert!(p(&["*", "a.lua"]).is_err());
        assert!(p(&["--bogus", "a.lua"]).is_err());
        assert!(p(&["-x", "a.lua"]).is_err());
        assert!(p(&["--layer", "middle", "a.lua"]).is_err());
        assert_eq!(p(&["-h"]), Ok(ParseResult::Help));
    }
}
