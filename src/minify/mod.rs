//! Size-oriented Lua minifier.

pub mod hoist;
pub mod printer;
pub mod rename;
pub mod resolve;
pub mod visit;

use crate::lua::ast::Block;
pub use printer::Style;

#[derive(Clone, Debug)]
pub struct Options {
    pub rename_locals: bool,
    pub rename_globals: bool,
    /// Cache API functions such as `screen.drawRect` in locals.
    pub hoist_api: bool,
    /// Cache repeated string and number literals in locals.
    pub hoist_constants: bool,
    pub style: Style,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            rename_locals: true,
            rename_globals: true,
            hoist_api: true,
            hoist_constants: true,
            style: Style::Compact,
        }
    }
}

impl Options {
    /// Only strips comments and whitespace.
    pub fn whitespace_only() -> Self {
        Options {
            rename_locals: false,
            rename_globals: false,
            hoist_api: false,
            hoist_constants: false,
            style: Style::Compact,
        }
    }
}

pub fn minify_ast(chunk: &mut Block, opts: &Options) -> String {
    // Hoisting introduces placeholder locals, so it requires local renaming.
    if opts.rename_locals && (opts.hoist_api || opts.hoist_constants) {
        let res = resolve::resolve(chunk);
        hoist::hoist(
            chunk,
            &res,
            &hoist::HoistOptions {
                api: opts.hoist_api,
                constants: opts.hoist_constants,
            },
        );
    }
    let res = resolve::resolve(chunk);
    let (locals, globals) = rename::plan(
        &res,
        &rename::RenameOptions {
            locals: opts.rename_locals,
            globals: opts.rename_globals,
        },
    );
    rename::apply(chunk, &locals, &globals);
    printer::print_chunk(chunk, opts.style)
}

#[cfg(test)]
mod tests;
