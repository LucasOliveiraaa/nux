//! Help pages: `nux help`, `nux help <command>` and `nux <command> --help`.
//!
//! Every command has a [`Page`]; the general help lists them from the same
//! table, so the two never disagree.

use std::fmt::Write;

/// A command with a help page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Init,
    Sync,
    Status,
    Install,
    Remove,
    Clean,
    Purge,
    Reset,
    Help,
}

impl Topic {
    pub const ALL: [Topic; 9] = [
        Topic::Init,
        Topic::Sync,
        Topic::Status,
        Topic::Install,
        Topic::Remove,
        Topic::Clean,
        Topic::Purge,
        Topic::Reset,
        Topic::Help,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|topic| topic.page().name == name)
    }

    pub fn name(self) -> &'static str {
        self.page().name
    }

    fn page(self) -> &'static Page {
        &PAGES[self as usize]
    }
}

struct Page {
    name: &'static str,
    /// What `nux help` lists: the command with its arguments.
    synopsis: &'static str,
    summary: &'static str,
    usage: &'static [&'static str],
    description: &'static str,
    options: &'static [Opt],
    examples: &'static [&'static str],
}

/// An option: its flags, and its description, one line per element.
type Opt = (&'static str, &'static [&'static str]);

const ENVIRONMENT: Opt = (
    "-e, --environment <path>",
    &[
        "Environment file, or a directory containing",
        "Nux.toml [default: ./Nux.toml, or the global",
        "environment when there is none]",
    ],
);
const GLOBAL: Opt = (
    "-g, --global",
    &["Use the global environment", "(~/.config/nux/Nux.toml)"],
);
const YES: Opt = ("-y, --yes", &["Do not ask for confirmation"]);
const WAIT: Opt = (
    "--wait",
    &[
        "Wait for another nux process using the",
        "workspace instead of failing",
    ],
);
const DRY_RUN: Opt = ("--dry-run", &["Only list what would be removed"]);
const HELP: Opt = ("-h, --help", &["Print this help"]);

/// In the order of [`Topic`].
const PAGES: [Page; 9] = [
    Page {
        name: "init",
        synopsis: "init [path]",
        summary: "Create an environment in `path` [default: .]",
        usage: &["nux init [path]"],
        description: "\
Creates `path` with a Nux.toml named after the directory, requesting
`hello`, and a starter recipe in repository/hello.nux that needs no
download, so the environment syncs right away. An existing Nux.toml is
never overwritten.",
        options: &[HELP],
        examples: &[
            "nux init",
            "nux init envs/school && nux sync -e envs/school",
        ],
    },
    Page {
        name: "sync",
        synopsis: "sync",
        summary: "Build every package an environment requests",
        usage: &["nux sync [-e <path> | -g] [-y] [--locked] [--wait]"],
        description: "\
Resolves the environment, downloads missing sources, builds what is not
installed (each build in a sandbox), and links the executables into
~/.nux/bin. Asks before building. Nux.lock is written when the resolution
or the sources change.",
        options: &[
            ENVIRONMENT,
            GLOBAL,
            (
                "--locked",
                &["Fail instead of creating or changing Nux.lock"],
            ),
            YES,
            WAIT,
            HELP,
        ],
        examples: &[
            "nux sync",
            "nux sync -e example/hello -y",
            "nux sync --locked -y   # in CI",
        ],
    },
    Page {
        name: "status",
        synopsis: "status",
        summary: "Show what is requested, installed and pending",
        usage: &["nux status [-e <path> | -g]"],
        description: "\
Shows the version selected for each requested package and dependency,
whether it is installed, the executable links, whether Nux.lock is up to
date, and what `nux sync` would do. Changes nothing.",
        options: &[ENVIRONMENT, GLOBAL, HELP],
        examples: &["nux status", "nux status -g"],
    },
    Page {
        name: "install",
        synopsis: "install <package>...",
        summary: "Request packages in an environment and sync it",
        usage: &["nux install <package>... [-e <path> | -g] [-y] [--wait]"],
        description: "\
Adds each package to the environment's [packages], keeping the file's
comments and layout, then syncs it. A package is `name` (any version, or
the requirement already there) or `name@requirement`. Nux.toml is only
written once the sync goes ahead. Outside of an environment, packages go
to the global one, which is created if needed.",
        options: &[ENVIRONMENT, GLOBAL, YES, WAIT, HELP],
        examples: &[
            "nux install jq",
            "nux install ripgrep@^14 \"cmake@>=4, <5\"",
            "nux install -g lazygit",
        ],
    },
    Page {
        name: "remove",
        synopsis: "remove <package>...",
        summary: "Remove packages from an environment and sync it",
        usage: &["nux remove <package>... [-e <path> | -g] [-y] [--wait]"],
        description: "\
Removes each package from the environment's [packages], then syncs it.
Its executable links go, unless another environment uses the same
version. Installations stay; `nux purge` removes the ones no environment
uses.",
        options: &[ENVIRONMENT, GLOBAL, YES, WAIT, HELP],
        examples: &["nux remove jq", "nux remove -g lazygit && nux purge"],
    },
    Page {
        name: "clean",
        synopsis: "clean",
        summary: "Remove leftovers from the Nux workspace",
        usage: &["nux clean [--sources] [--cache] [--dry-run] [-y] [--wait]"],
        description: "\
Removes interrupted downloads, build directories of failed builds, build
logs and temporary files. Installed packages are never removed.",
        options: &[
            (
                "--sources",
                &[
                    "Also remove downloaded sources and versions",
                    "that were never installed",
                ],
            ),
            (
                "--cache",
                &[
                    "Also empty the download cache (git mirrors,",
                    "downloaded files, shared build tool caches)",
                ],
            ),
            DRY_RUN,
            YES,
            WAIT,
            HELP,
        ],
        examples: &["nux clean --dry-run", "nux clean --sources -y"],
    },
    Page {
        name: "purge",
        synopsis: "purge",
        summary: "Remove packages no environment uses",
        usage: &["nux purge [--dry-run] [-y] [--wait]"],
        description: "\
Removes the package versions no environment uses any more, the links
into them, and the records of environments whose Nux.toml was deleted.
What environments use comes from their last sync.",
        options: &[DRY_RUN, YES, WAIT, HELP],
        examples: &["nux purge --dry-run", "nux purge -y"],
    },
    Page {
        name: "reset",
        synopsis: "reset",
        summary: "Remove everything from the Nux workspace",
        usage: &["nux reset [--keep-cache] [--dry-run] [-y] [--wait]"],
        description: "\
Starts over: removes every installed package and downloaded source, the
links Nux created, the environment records and the download cache. Files
Nux did not create are kept. Environments, their Nux.lock and the
configuration are not touched.",
        options: &[
            (
                "--keep-cache",
                &[
                    "Keep the download cache (git mirrors and",
                    "downloaded files)",
                ],
            ),
            DRY_RUN,
            YES,
            WAIT,
            HELP,
        ],
        examples: &["nux reset --dry-run", "nux reset --keep-cache -y"],
    },
    Page {
        name: "help",
        synopsis: "help [command]",
        summary: "Print this help, or a command's help",
        usage: &["nux help [command]", "nux <command> --help"],
        description: "\
Without a command, lists every command. With one, prints its usage,
options and examples.",
        options: &[],
        examples: &["nux help", "nux help sync", "nux install --help"],
    },
];

/// Where descriptions start in option and command lists.
const COLUMN: usize = 32;

/// `nux help`: every command, and how to learn more.
pub fn general() -> String {
    let mut text =
        String::from("nux - build and manage packages from .nux definitions\n\nUsage:\n");
    text.push_str("    nux <command> [options]\n    nux help [command]\n\nCommands:\n");
    for topic in Topic::ALL {
        let page = topic.page();
        row(&mut text, page.synopsis, &[page.summary]);
    }
    text.push_str("\nOptions:\n");
    row(&mut text, HELP.0, HELP.1);
    text.push_str(
        "\nRun `nux help <command>` or `nux <command> --help` for a command's options.\n",
    );
    text
}

/// `nux help <command>`.
pub fn page(topic: Topic) -> String {
    let page = topic.page();
    let mut text = format!(
        "nux {} - {}\n\nUsage:\n",
        page.name,
        lowercase_first(page.summary)
    );
    for usage in page.usage {
        let _ = writeln!(text, "    {usage}");
    }
    let _ = writeln!(text, "\n{}", page.description);
    if !page.options.is_empty() {
        text.push_str("\nOptions:\n");
        for (flags, description) in page.options {
            row(&mut text, flags, description);
        }
    }
    if !page.examples.is_empty() {
        text.push_str("\nExamples:\n");
        for example in page.examples {
            let _ = writeln!(text, "    {example}");
        }
    }
    text
}

/// `    <left>  <first line>`, further lines aligned under the first.
fn row(text: &mut String, left: &str, lines: &[&str]) {
    let left = format!("    {left}");
    for (index, line) in lines.iter().enumerate() {
        let lead = if index == 0 { left.as_str() } else { "" };
        let pad = if lead.len() + 2 > COLUMN {
            2
        } else {
            COLUMN - lead.len()
        };
        let _ = writeln!(text, "{lead}{}{line}", " ".repeat(pad));
    }
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_match_their_topics() {
        for topic in Topic::ALL {
            assert_eq!(Topic::from_name(topic.name()), Some(topic));
            assert!(page(topic).starts_with(&format!("nux {} - ", topic.name())));
        }
        assert_eq!(Topic::from_name("build"), None);
    }

    #[test]
    fn the_general_help_lists_every_command() {
        let text = general();
        for topic in Topic::ALL {
            assert!(
                text.contains(&format!("\n    {} ", topic.page().synopsis)),
                "{}",
                topic.name()
            );
        }
        assert!(
            text.contains(
                "    init [path]                 Create an environment in `path` [default: .]\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_command_page() {
        assert_eq!(
            page(Topic::Purge),
            "\
nux purge - remove packages no environment uses

Usage:
    nux purge [--dry-run] [-y] [--wait]

Removes the package versions no environment uses any more, the links
into them, and the records of environments whose Nux.toml was deleted.
What environments use comes from their last sync.

Options:
    --dry-run                   Only list what would be removed
    -y, --yes                   Do not ask for confirmation
    --wait                      Wait for another nux process using the
                                workspace instead of failing
    -h, --help                  Print this help

Examples:
    nux purge --dry-run
    nux purge -y
"
        );
    }

    #[test]
    fn lines_fit_in_80_columns() {
        for text in Topic::ALL.into_iter().map(page).chain([general()]) {
            for line in text.lines() {
                assert!(line.chars().count() <= 80, "{line}");
            }
        }
    }
}
