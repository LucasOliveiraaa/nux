# Lesson 9: Configuration and the sandbox

You already used `~/.config/nux/config.toml` to list repositories. In this
lesson you see everything else it can do, and how Nux protects your machine
from the builds it runs.

## The configuration file

Every setting is optional. Here they all are, with their defaults:

```toml
# ~/.config/nux/config.toml
[nux]
home = "~/.nux"           # where Nux installs everything
jobs = 4                  # recipes read at the same time
parallel_builds = 4       # packages built at the same time

# Recipe repositories every environment can use, after its own.
[[repository]]
path = "~/recipes"

[sandbox]
mode = "auto"             # "auto", "required" or "off"
network = true            # can builds use the network?
shared_caches = false     # share cargo and go downloads between builds?
```

The file lives in `$XDG_CONFIG_HOME/nux` if you set that variable, and in
`~/.config/nux` otherwise. Your global environment (`Nux.toml`, `Nux.lock`)
sits next to it.

### Where Nux installs

`home` moves the workspace, for example to a bigger disk. The
`NUX_HOME` environment variable overrides it for a single command:

```sh
NUX_HOME=/tmp/scratch nux sync     # try something without touching ~/.nux
```

Packages contain absolute paths, so after moving the workspace, packages are
rebuilt.

### Speed

- **`parallel_builds`** is how many packages Nux builds at the same time,
  when they don't depend on each other. Each build may use all your CPUs.
  Lower it (to 1, say) if several big builds at once use too much memory.
- **`jobs`** is how many recipe files Nux reads at the same time. Raise it if
  you have many recipes that call `exec`.

## The build sandbox

Recipes run commands, and you may not have written them all. So Nux runs
every build command in a *sandbox*. A build can:

- read the system (`/usr`, `/etc`...), so compilers and libraries work;
- write its own build directory and `$NUX_PREFIX`, and nothing else;
- use the network (unless you turn it off);
- use a private, empty home directory and `/tmp` for caches. They are
  discarded after each command.

A build cannot:

- see your home directory, so no SSH keys, browser profiles or documents;
- change anything outside its own directories, including other packages;
- leave processes running after it ends.

Programs you installed in your home, like those in `~/.cargo/bin` or
`~/.local/bin`, stay usable in builds, read-only, as long as their directory
is on your `PATH`.

Most recipes never notice the sandbox. When one does, the build fails with
`Read-only file system` or a missing file. Lesson 10 explains what to do.

### Sandbox settings

- **`mode`**:
  - `"auto"` (the default) sandboxes when possible. On systems that don't
    allow it, typically inside some containers, Nux warns and builds without
    a sandbox.
  - `"required"` refuses to build without one.
  - `"off"` never sandboxes.
- **`network = false`** cuts builds off from the network. Sources declared
  with `source(...)` still download, since Nux fetches them before building.
  But builds that download their own dependencies (Cargo, Go, pip) will fail.
- **`shared_caches = true`** lets builds share Cargo's and Go's download
  caches, instead of downloading every crate and module again for each
  build. It is faster, but a build could then tamper with what later builds
  get from these caches. Only turn it on if you trust every recipe you build.

## What you learned

- **`~/.config/nux/config.toml`** sets defaults for every environment:
  `home`, `jobs`, `parallel_builds`, repositories and the sandbox.
- **`NUX_HOME`** moves the workspace for one command.
- Builds run in a **sandbox**: the system is read-only, your home is hidden,
  and only the package's own directories are writable.
- **`[sandbox]`** settings control the sandbox: `mode`, `network` and
  `shared_caches`.

Next: [Troubleshooting](10-troubleshooting.md).
