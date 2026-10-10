# Learn Nux

Nux builds software from source and installs it in your home directory, with
no root access and without touching your system's package manager. You can
use it as a personal package manager (`nux install jq`), or to give each of
your projects the exact tools and libraries it needs.

These lessons teach you to use Nux step by step. Each one builds on the
previous one and takes 10 to 20 minutes. Run the commands as you read: every
example was run before it was written down, and the output you see in the
lessons is what Nux prints.

## Before you start

You need:

- Linux;
- Rust 1.85 or later, to build Nux (`rustc --version` to check);
- `git`, `curl`, a C compiler (`cc`) and `make`, which most systems have.

You don't need to know Rust. You should be comfortable with a terminal and
basic shell commands.

## The lessons

**Using Nux**

1. [Getting started](01-getting-started.md): install Nux, build your first
   package and run it.
2. [Installing packages](02-installing-packages.md): use Nux as a personal
   package manager with `install`, `remove` and `status`.
3. [Project environments](03-project-environments.md): give a project its own
   tools, pin their versions, and share them with your team.

**Writing recipes**

4. [Your first recipe](04-your-first-recipe.md): describe how to build a
   program, then build it.
5. [Packaging real software](05-packaging-real-software.md): download
   sources, use build-system helpers, and offer several versions.
6. [Dependencies and libraries](06-dependencies-and-libraries.md): build
   programs that need other programs and libraries.
7. [Using what your machine has](07-using-the-host.md): reuse installed
   compilers and tools instead of building them.

**Living with Nux**

8. [Housekeeping](08-housekeeping.md): free disk space, and start over when
   you need to.
9. [Configuration and the sandbox](09-configuration-and-sandbox.md): set
   defaults for every project, and understand what builds may do.
10. [Troubleshooting](10-troubleshooting.md): what common errors mean and how
    to fix them.

Once you know your way around, keep the [cheat sheet](cheatsheet.md) at
hand: commands, recipe functions and settings on one page.

The [`example/`](../example/README.md) folder has a ready-to-run environment
for each part of the lessons, and a repository of recipes you can use
right away.

## Getting help from Nux itself

Every command explains itself:

```sh
nux help              # every command
nux help install      # one command: usage, options and examples
nux sync --help       # the same, from the command
```
