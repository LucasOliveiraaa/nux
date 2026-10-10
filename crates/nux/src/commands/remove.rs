//! `nux remove`: stop requesting packages in an environment, then sync it.
//! Installations stay in the workspace, for other environments or a later
//! install; `nux purge` removes those no environment uses.

use std::io::{BufRead, Write};

use crate::commands::sync::{self, Edit, Io, Outcome};
use crate::environment::{Environment, Selection, without_packages};

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub packages: Vec<String>,
    pub environment: Selection,
    pub yes: bool,
    pub wait: bool,
}

pub fn run<W, R>(options: &Options, io: Io<W, R>) -> Result<Outcome, String>
where
    W: Write + Send + Clone,
    R: BufRead,
{
    let sync_options = sync::Options {
        environment: options.environment.clone(),
        yes: options.yes,
        locked: false,
        wait: options.wait,
    };
    sync::run_with(&sync_options, io, |config| {
        let current = Environment::select(&options.environment, config)?;
        let mut lines = Vec::new();
        for name in &options.packages {
            match current.packages.iter().find(|r| &r.name == name) {
                Some(request) => lines.push(format!("  {name} {}", request.version)),
                None => {
                    return Err(format!(
                        "`{name}` is not requested by environment `{}` ({})",
                        current.name,
                        current.path.display()
                    ));
                }
            }
        }
        let path = current.path.clone();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read environment `{}`: {e}", path.display()))?;
        let text = without_packages(&text, &options.packages)
            .map_err(|e| format!("cannot edit environment `{}`: {e}", path.display()))?;
        let env = Environment::from_text(&text, &path, current.global, config)?;
        let message = format!("Removing from `{}`:\n{}", path.display(), lines.join("\n"));
        Ok((env, Some(Edit { path, text: Some(text), message })))
    })
}
