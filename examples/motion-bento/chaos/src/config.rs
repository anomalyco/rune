//! The flags both builds share. Each build strips these and keeps the rest for its framework.

use std::path::PathBuf;

use crate::stress::Load;

#[derive(Clone, Debug)]
pub struct Config {
    /// Names one chaos schedule (and the random choices each action makes).
    pub seed: u64,
    /// Seconds of chaos. The window quits a second after it ends, writing metrics if asked.
    pub duration: f64,
    /// Scales how often things happen: 2 is twice the events.
    pub intensity: f64,
    /// Where to write the frame meter's JSON when the run ends.
    pub metrics: Option<PathBuf>,
    /// Show the frame meter over the window from the start (H toggles it).
    pub hud: bool,
    /// Write the schedule as a Rune script and exit.
    pub emit_script: Option<PathBuf>,
    /// A named look or spring tuning (`look::Variant`).
    pub variant: String,
    /// Keep the window open after the schedule ends instead of quitting.
    pub stay: bool,
    /// Stress mode (`--load N` or `--ramp A:B`): the wall of controls instead of the bento.
    pub load: Option<Load>,
    /// Where to write every frame's show time, load and times as CSV when the run ends.
    pub frames: Option<PathBuf>,
    /// Draw the stress HUD from this measured run's frames CSV instead of this run's own.
    pub hud_from: Option<PathBuf>,
    /// Build the whole view every frame, as a first port does, instead of each framework's
    /// idiomatic way (memoised cards and live values in Rune, per-card entities in GPUI).
    pub naive: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            seed: 1,
            duration: 20.0,
            intensity: 1.0,
            metrics: None,
            hud: false,
            emit_script: None,
            variant: "default".into(),
            stay: false,
            load: None,
            frames: None,
            hud_from: None,
            naive: false,
        }
    }
}

impl Config {
    /// Takes the bento's own flags out of `args` (the program name first) and returns them with the
    /// rest. `--duration` is read and also kept, since Rune's headless runs use it for video length.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<(Config, Vec<String>), String> {
        let mut config = Config::default();
        let mut rest = Vec::new();
        let mut args = args.into_iter();
        let (mut fixed, mut ramp, mut hold, mut timed) = (None, None, 2.5, false);
        fn value(flag: &str, next: Option<String>) -> Result<String, String> {
            next.ok_or_else(|| format!("{flag} needs a value"))
        }
        fn number<T: std::str::FromStr>(flag: &str, v: &str) -> Result<T, String> {
            v.parse().map_err(|_| format!("{flag}: not a number: {v}"))
        }
        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
                _ => (arg.clone(), None),
            };
            let take = |args: &mut dyn Iterator<Item = String>| match &inline {
                Some(v) => Ok(v.clone()),
                None => value(&flag, args.next()),
            };
            match flag.as_str() {
                "--seed" => config.seed = number(&flag, &take(&mut args)?)?,
                "--intensity" => {
                    config.intensity = number(&flag, &take(&mut args)?)?;
                    if !(config.intensity > 0.0) {
                        return Err("--intensity must be above 0".into());
                    }
                }
                "--duration" => {
                    let v = take(&mut args)?;
                    if v != "auto" {
                        config.duration = number(&flag, &v)?;
                    }
                    timed = true;
                    rest.push("--duration".into());
                    rest.push(v);
                }
                "--metrics" => config.metrics = Some(take(&mut args)?.into()),
                "--emit-script" => config.emit_script = Some(take(&mut args)?.into()),
                "--variant" => config.variant = take(&mut args)?,
                "--load" => fixed = Some(number::<usize>(&flag, &take(&mut args)?)?.max(1)),
                "--ramp" => ramp = Some(take(&mut args)?),
                "--hold" => hold = number(&flag, &take(&mut args)?)?,
                "--frames" => config.frames = Some(take(&mut args)?.into()),
                "--hud-from" => {
                    config.hud_from = Some(take(&mut args)?.into());
                    config.hud = true;
                }
                "--hud" => config.hud = true,
                "--stay" => config.stay = true,
                "--naive" => config.naive = true,
                _ => rest.push(arg),
            }
        }
        config.load = match (fixed, ramp) {
            (Some(_), Some(_)) => return Err("--load and --ramp: pick one".into()),
            (Some(n), None) => Some(Load::Fixed(n)),
            (None, Some(r)) => Some(Load::parse_ramp(&r, hold)?),
            (None, None) => None,
        };
        // A ramp runs as long as its levels, unless told otherwise; Rune's headless runs read it too.
        if let (false, Some(d)) = (timed, config.load.as_ref().and_then(Load::duration)) {
            config.duration = d;
            rest.push("--duration".into());
            rest.push(format!("{d}"));
        }
        Ok((config, rest))
    }

    /// The seconds between events on a quiet stretch, before intensity.
    pub fn gap(&self) -> (f64, f64) {
        (0.1 / self.intensity, 0.3 / self.intensity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn takes_its_own_flags_and_keeps_the_rest() {
        let (c, rest) =
            Config::parse(args("bento --seed 9 --snap a.png --intensity=2 --duration 4 --hud --naive --at 1")).unwrap();
        assert_eq!(c.seed, 9);
        assert!(c.naive);
        assert_eq!(c.intensity, 2.0);
        assert_eq!(c.duration, 4.0);
        assert!(c.hud);
        assert_eq!(rest, args("bento --snap a.png --duration 4 --at 1"));
    }

    #[test]
    fn a_ramp_sets_the_duration_unless_given() {
        let (c, rest) = Config::parse(args("bento --ramp 16:64 --hold 2")).unwrap();
        assert_eq!(c.load.as_ref().map(|l| l.levels().to_vec()), Some(vec![16, 23, 32, 45, 64]));
        assert_eq!(c.duration, 10.6);
        assert_eq!(rest, args("bento --duration 10.6"));
        let (c, _) = Config::parse(args("bento --load 40 --duration 3")).unwrap();
        assert_eq!((c.load, c.duration), (Some(Load::Fixed(40)), 3.0));
        assert!(Config::parse(args("bento --load 4 --ramp 1:2")).is_err());
    }

    #[test]
    fn rejects_a_bad_number() {
        assert!(Config::parse(args("bento --seed x")).is_err());
        assert!(Config::parse(args("bento --intensity 0")).is_err());
    }
}
