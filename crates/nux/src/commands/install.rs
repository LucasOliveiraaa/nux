//! `nux install`: request packages in an environment, then sync it.
//! Outside of any environment, packages go to the global one, which is
//! created if needed.

use std::io::{BufRead, Write};
use std::path::Path;

use nux_resolver::Request;

use crate::commands::sync::{self, Edit, Io, Outcome};
use crate::environment::{Environment, FILE_NAME, GLOBAL_TEMPLATE, Selection, with_packages};

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Each package with its requirement, if one was given.
    pub packages: Vec<(String, Option<String>)>,
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
        let mut selection = options.environment.clone();
        if selection.path.is_none() && !Path::new(FILE_NAME).exists() {
            selection.global = true;
        }
        let (path, global) = selection.locate(config)?;
        let (text, created) = match std::fs::read_to_string(&path) {
            Ok(text) => (text, false),
            Err(e) if global && e.kind() == std::io::ErrorKind::NotFound => {
                if config.repositories.is_empty() {
                    let file = config
                        .file()
                        .expect("the global environment is in the configuration directory");
                    return Err(format!(
                        "cannot create the global environment `{}`: no repositories configured; \
                         add a `[[repository]]` with a `path` to `{}`",
                        path.display(),
                        file.display()
                    ));
                }
                (GLOBAL_TEMPLATE.to_string(), true)
            }
            Err(e) => {
                return Err(format!(
                    "failed to read environment `{}`: {e}",
                    path.display()
                ));
            }
        };
        let current = Environment::from_text(&text, &path, global, config)?;

        let mut requests = Vec::new();
        let mut lines = Vec::new();
        for (name, requirement) in &options.packages {
            let existing = current
                .packages
                .iter()
                .find(|r| &r.name == name)
                .map(|r| r.version.as_str());
            match (requirement.as_deref(), existing) {
                (None, Some(old)) => lines.push(format!("  {name} {old} (already requested)")),
                (Some(new), Some(old)) if new == old => {
                    lines.push(format!("  {name} {old} (already requested)"))
                }
                (new, old) => {
                    let new = new.unwrap_or("latest");
                    lines.push(match old {
                        Some(old) => format!("  {name} {old} -> {new}"),
                        None => format!("  {name} {new}"),
                    });
                    requests.push(Request::new(name.clone(), new));
                }
            }
        }

        let heading = match (created, requests.is_empty()) {
            (true, _) => format!("Creating `{}`, requesting:", path.display()),
            (false, true) => format!("Already requested in `{}`:", path.display()),
            (false, false) => format!("Requesting in `{}`:", path.display()),
        };
        let (env, text) = if requests.is_empty() && !created {
            (current, None)
        } else {
            let text = with_packages(&text, &requests)
                .map_err(|e| format!("cannot edit environment `{}`: {e}", path.display()))?;
            (
                Environment::from_text(&text, &path, global, config)?,
                Some(text),
            )
        };
        let message = format!("{heading}\n{}", lines.join("\n"));
        Ok((
            env,
            Some(Edit {
                path,
                text,
                message,
            }),
        ))
    })
}
