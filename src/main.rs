mod bundle;
mod check;
mod config;
mod lua;
mod minify;
mod server;
mod sim;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use config::Config;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Stormworks microcontroller Lua toolkit: bundle + minify, lint, and simulate.
#[derive(Parser)]
#[command(name = "swlua", version)]
struct Cli {
    /// Path to swlua.toml (default: search the current directory and parents).
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new project with an example script, test bench and editor types.
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Monitor size for the simulator, e.g. 3x2.
        #[arg(long, default_value = "2x2")]
        screen: String,
    },
    /// Bundle `require`d modules, minify and write the project's output file.
    Build {
        #[command(flatten)]
        min: MinifyArgs,
        /// Output file (default: [project] out).
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        /// Rebuild whenever a source file changes.
        #[arg(long, short = 'w')]
        watch: bool,
    },
    /// Minify a single file (bundling its requires) to stdout or a file.
    Minify {
        /// Input file, or - for stdin.
        file: PathBuf,
        #[command(flatten)]
        min: MinifyArgs,
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
    },
    /// Lint a script for things that do not work in a microcontroller.
    Check {
        /// Script to check (default: [project] main).
        file: Option<PathBuf>,
    },
    /// Run the simulator headless for a number of ticks.
    Run {
        #[command(flatten)]
        sim: SimArgs,
        /// Number of ticks to simulate (60 per second).
        #[arg(long, short = 't', default_value_t = 600)]
        ticks: u64,
        /// Write outputs for every tick to a CSV file.
        #[arg(long)]
        csv: Option<PathBuf>,
        /// Save the final monitor frame as PNG.
        #[arg(long)]
        screenshot: Option<PathBuf>,
        /// Save a PNG frame every N ticks into --frames-dir.
        #[arg(long, requires = "frames_dir")]
        frame_every: Option<u64>,
        #[arg(long)]
        frames_dir: Option<PathBuf>,
        /// Print the outputs at the end.
        #[arg(long, short = 'p')]
        print_outputs: bool,
    },
    /// Interactive simulator/debugger in the browser, reloading on save.
    Sim {
        #[command(flatten)]
        sim: SimArgs,
        #[arg(long, default_value = "127.0.0.1:7272")]
        listen: String,
        /// Start paused.
        #[arg(long)]
        paused: bool,
    },
}

#[derive(Args, Clone, Default)]
struct MinifyArgs {
    /// Keep local variable names.
    #[arg(long)]
    keep_locals: bool,
    /// Keep global names defined by the script.
    #[arg(long)]
    keep_globals: bool,
    /// Do not cache API functions like screen.drawRect in locals.
    #[arg(long)]
    no_hoist: bool,
    /// Do not deduplicate repeated string/number literals.
    #[arg(long)]
    no_constants: bool,
    /// Only strip comments and whitespace.
    #[arg(long)]
    whitespace_only: bool,
    /// Indented output (larger, for debugging the minifier).
    #[arg(long)]
    pretty: bool,
    /// Only bundle requires; do not minify at all.
    #[arg(long)]
    no_minify: bool,
}

#[derive(Args, Clone)]
struct SimArgs {
    /// Script to run (default: [project] main).
    file: Option<PathBuf>,
    /// Monitor size, e.g. 3x2 or 96x64 (default: [sim] screen).
    #[arg(long, short = 's')]
    screen: Option<String>,
    /// Test bench script (default: [sim] bench).
    #[arg(long, short = 'b')]
    bench: Option<PathBuf>,
    /// Run the minified build instead of the source, to verify the output.
    #[arg(long, short = 'm')]
    minified: bool,
    /// Do not feed monitor touch data into inputs 1-6 / bools 1-2.
    #[arg(long)]
    no_touch: bool,
    /// Set a property: --prop "Max Speed=12" (number, true/false or text).
    #[arg(long = "prop", value_name = "NAME=VALUE")]
    props: Vec<String>,
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn load_config(cli: &Cli) -> Result<Option<Config>> {
    match &cli.config {
        Some(p) => Config::load(p).map(Some),
        None => Config::find(Path::new(".")),
    }
}

fn run(cli: Cli) -> Result<()> {
    let cfg = load_config(&cli)?;
    match cli.cmd {
        Cmd::Init { dir, screen } => init(&dir, &screen),
        Cmd::Build { min, out, watch } => {
            let cfg = cfg.context(
                "no swlua.toml found (run `swlua init` first, or use `swlua minify <file>`)",
            )?;
            if watch {
                return watch_loop(|| build(&cfg, &min, out.as_deref()));
            }
            build(&cfg, &min, out.as_deref()).map(|_| ())
        }
        Cmd::Minify { file, min, out } => {
            let opts = minify_options(&min, cfg.as_ref());
            let code = if file.as_os_str() == "-" {
                let mut src = String::new();
                std::io::stdin().read_to_string(&mut src)?;
                let mut chunk =
                    lua::parser::parse(&src).map_err(|e| anyhow::anyhow!("stdin:{e}"))?;
                render(&mut chunk, &min, &opts)
            } else {
                let lib = cfg.as_ref().map(|c| c.lib_dirs()).unwrap_or_default();
                let (mut chunk, _) = bundle::Bundler::new(lib).bundle(&file)?;
                render(&mut chunk, &min, &opts)
            };
            match out {
                Some(p) => write_out(&p, &code)?,
                None => std::io::stdout().write_all(code.as_bytes())?,
            }
            let limit = cfg
                .as_ref()
                .map_or(config::DEFAULT_LIMIT, |c| c.project.limit);
            report_size(code.chars().count(), limit);
            Ok(())
        }
        Cmd::Check { file } => {
            let main = match (file, &cfg) {
                (Some(f), _) => f,
                (None, Some(c)) => c.path(&c.project.main),
                (None, None) => bail!("no file given and no swlua.toml found"),
            };
            let lib = cfg.as_ref().map(|c| c.lib_dirs()).unwrap_or_default();
            let (chunk, files) = bundle::Bundler::new(lib).bundle(&main)?;
            let diags = check::check(&chunk, false);
            for d in &diags {
                if d.line == 0 {
                    println!("warning: {}", d.msg);
                } else {
                    println!("warning: line {}: {}", d.line, d.msg);
                }
            }
            if files.len() > 1 {
                println!(
                    "note: line numbers refer to the file each name first appears in ({} files bundled)",
                    files.len()
                );
            }
            let opts = minify_options(&MinifyArgs::default(), cfg.as_ref());
            let code = minify::minify_ast(&mut chunk.clone(), &opts);
            let limit = cfg
                .as_ref()
                .map_or(config::DEFAULT_LIMIT, |c| c.project.limit);
            report_size(code.chars().count(), limit);
            if diags.is_empty() {
                println!("no problems found");
            }
            Ok(())
        }
        Cmd::Run {
            sim,
            ticks,
            csv,
            screenshot,
            frame_every,
            frames_dir,
            print_outputs,
        } => {
            let (factory, _, _) = sim_factory(&sim, cfg.as_ref())?;
            let mut s = factory()?;
            let mut csv_out = match &csv {
                Some(p) => {
                    let mut f = std::io::BufWriter::new(std::fs::File::create(p)?);
                    let mut head = vec!["tick".to_string()];
                    head.extend((1..=32).map(|i| format!("n{i}")));
                    head.extend((1..=32).map(|i| format!("b{i}")));
                    writeln!(f, "{}", head.join(","))?;
                    Some(f)
                }
                None => None,
            };
            if let Some(d) = &frames_dir {
                std::fs::create_dir_all(d)?;
            }
            let mut failed = None;
            for _ in 0..ticks {
                let r = s.step();
                for l in s.take_logs() {
                    println!("{l}");
                }
                if let Err(e) = r {
                    failed = Some(e);
                    break;
                }
                let st = s.st.borrow();
                if let Some(f) = csv_out.as_mut() {
                    let mut row = vec![(s.tick - 1).to_string()];
                    row.extend(st.out_num.iter().map(|v| v.to_string()));
                    row.extend(st.out_bool.iter().map(|b| (*b as u8).to_string()));
                    writeln!(f, "{}", row.join(","))?;
                }
                drop(st);
                if let (Some(every), Some(dir)) = (frame_every, &frames_dir)
                    && (s.tick - 1) % every.max(1) == 0
                    && let Some(png) = s.screen_png()
                {
                    std::fs::write(dir.join(format!("frame_{:06}.png", s.tick - 1)), png)?;
                }
                if std::mem::take(&mut s.st.borrow_mut().stop_requested) {
                    break;
                }
            }
            if let Some(mut f) = csv_out {
                f.flush()?;
            }
            if let Some(p) = &screenshot {
                match s.screen_png() {
                    Some(png) => std::fs::write(p, png)?,
                    None => eprintln!("warning: no screen configured; use --screen 3x2"),
                }
            }
            if print_outputs {
                let st = s.st.borrow();
                for i in 0..32 {
                    if st.out_num[i] != 0.0 || st.out_bool[i] {
                        println!(
                            "ch{:<2} number={:<12} bool={}",
                            i + 1,
                            st.out_num[i],
                            st.out_bool[i]
                        );
                    }
                }
            }
            eprintln!(
                "simulated {} ticks ({:.1} s game time)",
                s.tick,
                s.tick as f64 / 60.0
            );
            if let Some(e) = failed {
                bail!("script error at tick {}:\n{e}", s.tick);
            }
            Ok(())
        }
        Cmd::Sim {
            sim,
            listen,
            paused,
        } => {
            let (factory, tick_rate, watch) = sim_factory(&sim, cfg.as_ref())?;
            server::serve(factory, watch, &listen, tick_rate, paused)
        }
    }
}

fn minify_options(a: &MinifyArgs, cfg: Option<&Config>) -> minify::Options {
    let m = cfg.map(|c| &c.minify);
    let d = config::Minify::default();
    let m = m.unwrap_or(&d);
    if a.whitespace_only {
        return minify::Options {
            style: if a.pretty {
                minify::Style::Pretty
            } else {
                minify::Style::Compact
            },
            ..minify::Options::whitespace_only()
        };
    }
    minify::Options {
        rename_locals: m.rename_locals && !a.keep_locals,
        rename_globals: m.rename_globals && !a.keep_globals,
        hoist_api: m.hoist_api && !a.no_hoist,
        hoist_constants: m.hoist_constants && !a.no_constants,
        style: if a.pretty {
            minify::Style::Pretty
        } else {
            minify::Style::Compact
        },
    }
}

fn render(chunk: &mut lua::ast::Block, a: &MinifyArgs, opts: &minify::Options) -> String {
    if a.no_minify {
        let mut s = minify::printer::print_chunk(chunk, minify::Style::Pretty);
        s.push('\n');
        return s;
    }
    minify::minify_ast(chunk, opts)
}

/// Bundles and minifies the project; returns the input files.
fn build(cfg: &Config, a: &MinifyArgs, out: Option<&Path>) -> Result<Vec<PathBuf>> {
    let main = cfg.path(&cfg.project.main);
    let (mut chunk, files) = bundle::Bundler::new(cfg.lib_dirs()).bundle(&main)?;
    let before: usize = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .map(|s| s.chars().count())
        .sum();
    let code = render(&mut chunk, a, &minify_options(a, Some(cfg)));
    let out = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cfg.path(&cfg.project.out));
    write_out(&out, &code)?;
    let n = code.chars().count();
    eprintln!(
        "built {} from {} file(s): {} -> {} chars",
        out.display(),
        files.len(),
        before,
        n
    );
    report_size(n, cfg.project.limit);
    Ok(files)
}

fn write_out(p: &Path, code: &str) -> Result<()> {
    if let Some(d) = p.parent()
        && !d.as_os_str().is_empty()
    {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, code).with_context(|| format!("writing {}", p.display()))
}

fn report_size(n: usize, limit: usize) {
    let pct = n as f64 * 100.0 / limit as f64;
    if n > limit {
        eprintln!(
            "warning: {n} chars is over the {limit} char limit by {} ({pct:.0}%)",
            n - limit
        );
    } else {
        eprintln!("size: {n}/{limit} chars ({pct:.0}%)");
    }
}

fn watch_loop(mut f: impl FnMut() -> Result<Vec<PathBuf>>) -> Result<()> {
    let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut watched: Vec<(PathBuf, Option<std::time::SystemTime>)> = Vec::new();
    let mut run_once = |watched: &mut Vec<_>| match f() {
        Ok(files) => *watched = files.iter().map(|p| (p.clone(), mtime(p))).collect(),
        Err(e) => {
            eprintln!("error: {e:#}");
            // Keep watching the previous set so a fix triggers a rebuild.
            for w in watched.iter_mut() {
                w.1 = mtime(&w.0);
            }
        }
    };
    run_once(&mut watched);
    eprintln!("watching for changes (Ctrl+C to stop)");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(300));
        if watched.iter().any(|(p, t)| mtime(p) != *t) {
            run_once(&mut watched);
        }
    }
}

fn parse_prop(s: &str) -> Result<(String, sim::PropValue)> {
    let (k, v) = s
        .split_once('=')
        .with_context(|| format!("--prop expects NAME=VALUE, got '{s}'"))?;
    let v = v.trim();
    let val = match v {
        "true" => sim::PropValue::Bool(true),
        "false" => sim::PropValue::Bool(false),
        _ => match v.parse::<f64>() {
            Ok(n) => sim::PropValue::Number(n),
            Err(_) => sim::PropValue::Text(v.to_string()),
        },
    };
    Ok((k.trim().to_string(), val))
}

/// Returns a constructor for fresh simulator instances, the tick rate and the
/// files to watch for changes.
fn sim_factory(a: &SimArgs, cfg: Option<&Config>) -> Result<(server::Factory, f64, Vec<PathBuf>)> {
    let default_sim = config::Sim::default();
    let sc = cfg.map_or(&default_sim, |c| &c.sim);
    let rel = |p: &Path| cfg.map_or(p.to_path_buf(), |c| c.path(p));
    let main = match (&a.file, cfg) {
        (Some(f), _) => f.clone(),
        (None, Some(c)) => c.path(&c.project.main),
        (None, None) => bail!("no script given and no swlua.toml found"),
    };
    let screen = match a.screen.as_ref().or(sc.screen.as_ref()) {
        Some(s) if s != "none" => Some(config::parse_screen(s)?),
        _ => None,
    };
    let bench = a
        .bench
        .clone()
        .or_else(|| sc.bench.as_ref().map(|p| rel(p)));
    let mut props = sim::props_from_toml(&sc.properties)?;
    for p in &a.props {
        let (k, v) = parse_prop(p)?;
        props.insert(k, v);
    }
    let parse_ch = |k: &String| {
        k.parse::<usize>()
            .with_context(|| format!("[sim] channel '{k}' is not a number"))
    };
    let numbers: HashMap<usize, f64> = sc
        .numbers
        .iter()
        .map(|(k, v)| Ok((parse_ch(k)?, *v)))
        .collect::<Result<_>>()?;
    let bools: HashMap<usize, bool> = sc
        .bools
        .iter()
        .map(|(k, v)| Ok((parse_ch(k)?, *v)))
        .collect::<Result<_>>()?;
    let touch = sc.touch && !a.no_touch;
    let lib = cfg.map(|c| c.lib_dirs()).unwrap_or_default();
    let mut watch = vec![main.clone()];
    watch.extend(bench.clone());
    let minified = a.minified;
    let min_opts = minify_options(&MinifyArgs::default(), cfg);
    let factory: server::Factory = Box::new(move || {
        let opts = sim::SimOptions {
            screen,
            touch,
            props: props.clone(),
            numbers: numbers.clone(),
            bools: bools.clone(),
            bench: bench.clone(),
        };
        if minified {
            let (mut chunk, files) = bundle::Bundler::new(lib.clone()).bundle(&main)?;
            let code = minify::minify_ast(&mut chunk, &min_opts);
            let s = sim::Sim::new(
                sim::Source::Code {
                    name: "minified".into(),
                    code,
                },
                opts,
            )?;
            s.files.borrow_mut().extend(files);
            Ok(s)
        } else {
            sim::Sim::new(
                sim::Source::File {
                    main: main.clone(),
                    search: lib.clone(),
                },
                opts,
            )
        }
    });
    Ok((factory, sc.tick_rate, watch))
}

fn init(dir: &Path, screen: &str) -> Result<()> {
    config::parse_screen(screen)?;
    let cfg_path = dir.join(config::FILE_NAME);
    if cfg_path.exists() {
        bail!("{} already exists", cfg_path.display());
    }
    let files: &[(&str, &str)] = &[
        ("src/main.lua", include_str!("../templates/main.lua")),
        ("src/util.lua", include_str!("../templates/util.lua")),
        ("bench.lua", include_str!("../templates/bench.lua")),
        (
            ".swlua/stormworks.lua",
            include_str!("../templates/stormworks.lua"),
        ),
    ];
    for (p, content) in files {
        let path = dir.join(p);
        if path.exists() {
            eprintln!("skipped {} (exists)", path.display());
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, content)?;
        eprintln!("created {}", path.display());
    }
    let toml = format!(
        r#"[project]
main = "src/main.lua"
out = "out/main.lua"
# Extra folders searched by require("name"), e.g. a shared library.
lib = []
limit = {limit}

[minify]
rename_locals = true
rename_globals = true
hoist_api = true
hoist_constants = true

[sim]
screen = "{screen}"
bench = "bench.lua"
touch = true

[sim.properties]
Gain = 1

# Constant input values by channel.
[sim.numbers]

[sim.bools]
"#,
        limit = config::DEFAULT_LIMIT
    );
    std::fs::write(&cfg_path, toml)?;
    eprintln!("created {}", cfg_path.display());
    let luarc = dir.join(".luarc.json");
    if !luarc.exists() {
        std::fs::write(
            &luarc,
            r#"{
  "$schema": "https://raw.githubusercontent.com/LuaLS/vscode-lua/master/setting/schema.json",
  "runtime.version": "Lua 5.3",
  "runtime.builtin": { "io": "disable", "os": "disable", "package": "disable", "debug": "disable", "coroutine": "disable", "utf8": "disable" },
  "workspace.library": [".swlua"],
  "workspace.ignoreDir": ["out"],
  "diagnostics.globals": ["sim", "onSimInit", "onSimTick", "onSimAfterTick"]
}
"#,
        )?;
        eprintln!("created {}", luarc.display());
    }
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        std::fs::write(&gi, "out/\n")?;
    }
    eprintln!(
        "\nnext: cd {} && swlua sim   (then open http://127.0.0.1:7272)",
        dir.display()
    );
    Ok(())
}
