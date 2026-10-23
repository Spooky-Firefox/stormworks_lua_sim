//! `swlua.toml` project configuration.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "swlua.toml";

/// Character limit of a microcontroller Lua script.
pub const DEFAULT_LIMIT: usize = 8192;

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub project: Project,
    #[serde(default)]
    pub minify: Minify,
    #[serde(default)]
    pub sim: Sim,
    /// Directory containing the config file; paths are relative to it.
    #[serde(skip)]
    pub root: PathBuf,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields, default)]
pub struct Project {
    pub main: PathBuf,
    pub out: PathBuf,
    /// Extra directories searched by `require`.
    pub lib: Vec<PathBuf>,
    pub limit: usize,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            main: "src/main.lua".into(),
            out: "out/main.lua".into(),
            lib: vec![],
            limit: DEFAULT_LIMIT,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields, default)]
pub struct Minify {
    pub rename_locals: bool,
    pub rename_globals: bool,
    pub hoist_api: bool,
    pub hoist_constants: bool,
}

impl Default for Minify {
    fn default() -> Self {
        Minify {
            rename_locals: true,
            rename_globals: true,
            hoist_api: true,
            hoist_constants: true,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields, default)]
pub struct Sim {
    /// Monitor size as blocks ("3x2") or pixels ("96x64"). None = no screen.
    pub screen: Option<String>,
    /// Test bench script with `onSimInit` / `onSimTick` / `onSimAfterTick`.
    pub bench: Option<PathBuf>,
    /// Feed monitor touch data into composite inputs like a real monitor.
    pub touch: bool,
    pub tick_rate: f64,
    pub properties: BTreeMap<String, toml::Value>,
    /// Constant input values, keyed by channel ("1".."32").
    pub numbers: BTreeMap<String, f64>,
    pub bools: BTreeMap<String, bool>,
}

impl Default for Sim {
    fn default() -> Self {
        Sim {
            screen: None,
            bench: None,
            touch: true,
            tick_rate: 60.0,
            properties: BTreeMap::new(),
            numbers: BTreeMap::new(),
            bools: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Looks for `swlua.toml` in `start` and its parents.
    pub fn find(start: &Path) -> Result<Option<Config>> {
        let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
        for dir in start.ancestors() {
            let p = dir.join(FILE_NAME);
            if p.is_file() {
                return Config::load(&p).map(Some);
            }
        }
        Ok(None)
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(cfg)
    }

    pub fn path(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        }
    }

    pub fn lib_dirs(&self) -> Vec<PathBuf> {
        self.project.lib.iter().map(|p| self.path(p)).collect()
    }
}

/// (blocks wide, blocks high) -> (pixels wide, pixels high)
type MonitorSize = ((u32, u32), (usize, usize));

/// Monitor sizes in blocks and their resolution in pixels.
pub const MONITORS: &[MonitorSize] = &[
    ((1, 1), (32, 32)),
    ((2, 1), (64, 32)),
    ((2, 2), (64, 64)),
    ((3, 2), (96, 64)),
    ((3, 3), (96, 96)),
    ((5, 3), (160, 96)),
    ((9, 5), (288, 160)),
];

/// Parses "3x2" (monitor blocks) or "96x64" (pixels).
pub fn parse_screen(s: &str) -> Result<(usize, usize)> {
    let Some((a, b)) = s
        .to_ascii_lowercase()
        .split_once('x')
        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
    else {
        bail!("screen size must look like 3x2 or 96x64, got '{s}'");
    };
    let (w, h): (usize, usize) = (a.parse()?, b.parse()?);
    if let Some((_, px)) = MONITORS
        .iter()
        .find(|((bw, bh), _)| *bw as usize == w && *bh as usize == h)
    {
        return Ok(*px);
    }
    if w < 16 || h < 16 {
        let sizes: Vec<String> = MONITORS
            .iter()
            .map(|((w, h), _)| format!("{w}x{h}"))
            .collect();
        bail!("unknown monitor size '{s}' (known: {})", sizes.join(", "));
    }
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens() {
        assert_eq!(parse_screen("3x2").unwrap(), (96, 64));
        assert_eq!(parse_screen("96x64").unwrap(), (96, 64));
        assert!(parse_screen("4x4").is_err());
    }

    #[test]
    fn parses_config() {
        let cfg: Config = toml::from_str(
            r#"
            [project]
            main = "src/main.lua"
            limit = 4096
            [sim]
            screen = "2x2"
            [sim.properties]
            "Max Speed" = 12.5
            Enabled = true
            [sim.numbers]
            1 = 3.0
            "#,
        )
        .unwrap();
        assert_eq!(cfg.project.limit, 4096);
        assert_eq!(cfg.sim.numbers["1"], 3.0);
    }
}
