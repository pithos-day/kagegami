use std::path::PathBuf;

pub const USAGE: &str = "usage: kagegami [-o OUTPUT] [-f FPS] [-s SCALE] [-l background|bottom|top|overlay] <wallpaper.lua|wallpaper.glsl>";

/// `--scale` で受け付ける範囲。
pub const SCALE_MIN: f64 = 0.25;
pub const SCALE_MAX: f64 = 2.0;

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
    /// 出力の解像度に対する、描画する解像度の倍率。
    pub render_scale: f64,
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
    let mut render_scale = 1.0;
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
            "-s" | "--scale" => {
                let v = iter.next().ok_or(format!("{arg} requires a value"))?;
                render_scale = v
                    .parse::<f64>()
                    .ok()
                    .filter(|s| (SCALE_MIN..=SCALE_MAX).contains(s))
                    .ok_or_else(|| {
                        format!("invalid {arg} value: {v} (must be {SCALE_MIN} to {SCALE_MAX})")
                    })?;
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
        Err(p) if p.is_empty() => return Err("missing <wallpaper.lua|wallpaper.glsl>".into()),
        Err(_) => {
            return Err(
                "expected only one wallpaper file (choose an output with -o NAME; all outputs by default)"
                    .into(),
            );
        }
    };
    Ok(ParseResult::Run(Args {
        fps,
        render_scale,
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
                render_scale: 1.0,
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
    fn parses_scale_within_range() {
        assert_eq!(run(&["-s", "0.5", "a.lua"]).render_scale, 0.5);
        assert_eq!(run(&["--scale", "0.25", "a.lua"]).render_scale, 0.25);
        assert_eq!(run(&["-s", "2", "a.lua"]).render_scale, 2.0);
    }

    #[test]
    fn rejects_scale_out_of_range() {
        assert!(p(&["-s", "0.24", "a.lua"]).is_err());
        assert!(p(&["-s", "2.01", "a.lua"]).is_err());
        assert!(p(&["-s", "0", "a.lua"]).is_err());
        assert!(p(&["-s", "-1", "a.lua"]).is_err());
        assert!(p(&["-s", "NaN", "a.lua"]).is_err());
        assert!(p(&["-s", "inf", "a.lua"]).is_err());
        assert!(p(&["-s", "half", "a.lua"]).is_err());
        assert!(p(&["-s"]).is_err());
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
