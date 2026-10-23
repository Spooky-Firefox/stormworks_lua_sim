//! Inlines `require("module")` calls so a multi-file project becomes the
//! single script a microcontroller accepts.
//!
//! Modules are emitted once, in dependency order, ahead of the main script:
//! * `require "m"` used as a statement becomes `do <m> end`;
//! * `local x = require "m"` evaluates the module once into a generated
//!   global, `__req_m = (function(...) <m> end)()`, and every use refers to it.

use crate::lua::ast::*;
use crate::lua::parser::parse;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Bundler {
    search: Vec<PathBuf>,
    modules: HashMap<String, Module>,
    order: Vec<String>,
    loading: Vec<String>,
}

struct Module {
    path: PathBuf,
    body: Block,
    as_value: bool,
}

/// Finds `name` (dotted or slashed) in the search path.
pub fn find_module(search: &[PathBuf], name: &str) -> Option<PathBuf> {
    let rel = name.replace('.', "/");
    let rel = rel.trim_end_matches("/lua");
    for dir in search {
        for cand in [
            dir.join(format!("{rel}.lua")),
            dir.join(rel).join("init.lua"),
        ] {
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

pub fn global_for(module: &str) -> String {
    let clean: String = module
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("__req_{clean}")
}

impl Bundler {
    pub fn new(search: Vec<PathBuf>) -> Self {
        Bundler {
            search,
            modules: HashMap::new(),
            order: Vec::new(),
            loading: Vec::new(),
        }
    }

    /// Returns the bundled chunk and the list of files it was built from.
    pub fn bundle(mut self, main: &Path) -> Result<(Block, Vec<PathBuf>)> {
        let src =
            std::fs::read_to_string(main).with_context(|| format!("reading {}", main.display()))?;
        let mut body = parse(&src).map_err(|e| anyhow::anyhow!("{}:{e}", main.display()))?;
        let mut search = vec![main.parent().unwrap_or(Path::new(".")).to_path_buf()];
        search.extend(self.search.iter().cloned());
        self.search = search;
        self.rewrite_block(&mut body, main)?;

        let mut chunk = Vec::new();
        let mut files = vec![main.to_path_buf()];
        for name in &self.order {
            let m = self.modules.remove(name).unwrap();
            files.push(m.path);
            let has_return = m.body.iter().any(|s| matches!(s, Stat::Return(_)));
            if m.as_value || has_return {
                let f = Expr::Function(Box::new(FuncBody {
                    params: vec![],
                    vararg: true,
                    body: m.body,
                }));
                let call = Expr::Call(Box::new(Expr::Paren(Box::new(f))), vec![]);
                if m.as_value {
                    chunk.push(Stat::Assign(
                        vec![Expr::Name(Name::new(global_for(name), 0))],
                        vec![call],
                    ));
                } else {
                    chunk.push(Stat::Call(call));
                }
            } else {
                chunk.push(Stat::Do(m.body));
            }
        }
        chunk.extend(body);
        Ok((chunk, files))
    }

    fn load(&mut self, name: &str, from: &Path, as_value: bool) -> Result<()> {
        if let Some(m) = self.modules.get_mut(name) {
            m.as_value |= as_value;
            return Ok(());
        }
        if self.loading.iter().any(|n| n == name) {
            bail!("circular require: {} -> {name}", self.loading.join(" -> "));
        }
        let Some(path) = find_module(&self.search, name) else {
            let dirs: Vec<String> = self
                .search
                .iter()
                .map(|d| d.display().to_string())
                .collect();
            bail!(
                "{}: module '{name}' not found (searched {})",
                from.display(),
                dirs.join(", ")
            );
        };
        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut body = parse(&src).map_err(|e| anyhow::anyhow!("{}:{e}", path.display()))?;
        self.loading.push(name.to_string());
        self.rewrite_block(&mut body, &path)?;
        self.loading.pop();
        self.order.push(name.to_string());
        self.modules.insert(
            name.to_string(),
            Module {
                path,
                body,
                as_value,
            },
        );
        Ok(())
    }

    fn rewrite_block(&mut self, b: &mut Block, file: &Path) -> Result<()> {
        let mut err = None;
        b.retain_mut(|st| {
            if err.is_some() {
                return true;
            }
            if let Stat::Call(e) = st
                && let Some(name) = require_target(e)
            {
                if let Err(e) = self.load(&name, file, false) {
                    err = Some(e);
                }
                return false;
            }
            if let Err(e) = self.rewrite_stat(st, file) {
                err = Some(e);
            }
            true
        });
        err.map_or(Ok(()), Err)
    }

    fn rewrite_stat(&mut self, st: &mut Stat, file: &Path) -> Result<()> {
        match st {
            Stat::Call(e) => self.rewrite_expr(e, file),
            Stat::Assign(t, v) => {
                self.rewrite_exprs(t, file)?;
                self.rewrite_exprs(v, file)
            }
            Stat::Local(_, v) | Stat::Return(v) => self.rewrite_exprs(v, file),
            Stat::LocalFunction(_, f) | Stat::Function(_, _, f) => {
                self.rewrite_block(&mut f.body, file)
            }
            Stat::Do(b) => self.rewrite_block(b, file),
            Stat::While(c, b) | Stat::Repeat(b, c) => {
                self.rewrite_expr(c, file)?;
                self.rewrite_block(b, file)
            }
            Stat::If(cl, els) => {
                for (c, b) in cl.iter_mut() {
                    self.rewrite_expr(c, file)?;
                    self.rewrite_block(b, file)?;
                }
                match els {
                    Some(b) => self.rewrite_block(b, file),
                    None => Ok(()),
                }
            }
            Stat::NumFor(_, a, b, c, body) => {
                self.rewrite_expr(a, file)?;
                self.rewrite_expr(b, file)?;
                if let Some(c) = c {
                    self.rewrite_expr(c, file)?;
                }
                self.rewrite_block(body, file)
            }
            Stat::GenFor(_, v, body) => {
                self.rewrite_exprs(v, file)?;
                self.rewrite_block(body, file)
            }
            Stat::Break | Stat::Goto(_) | Stat::Label(_) => Ok(()),
        }
    }

    fn rewrite_exprs(&mut self, es: &mut [Expr], file: &Path) -> Result<()> {
        es.iter_mut().try_for_each(|e| self.rewrite_expr(e, file))
    }

    fn rewrite_expr(&mut self, e: &mut Expr, file: &Path) -> Result<()> {
        if let Some(name) = require_target(e) {
            self.load(&name, file, true)?;
            *e = Expr::Name(Name::new(global_for(&name), 0));
            return Ok(());
        }
        if let Expr::Function(f) = e {
            return self.rewrite_block(&mut f.body, file);
        }
        let mut res = Ok(());
        crate::minify::visit::expr_children(e, &mut |c| {
            if res.is_ok() {
                res = self.rewrite_expr(c, file);
            }
        });
        res
    }
}

fn require_target(e: &Expr) -> Option<String> {
    let Expr::Call(f, args) = e else { return None };
    let Expr::Name(n) = f.as_ref() else {
        return None;
    };
    if n.text != "require" {
        return None;
    }
    match args.as_slice() {
        [Expr::Str(s)] => String::from_utf8(s.clone()).ok(),
        _ => None,
    }
}
