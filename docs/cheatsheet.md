# Cheat sheet

Everything on one page, to look things up once you've followed the
[lessons](README.md).

## Commands

| Command | Does |
| --- | --- |
| `nux init [dir]` | Create an environment with a starter recipe. |
| `nux sync` | Build and link everything the environment asks for. |
| `nux status` | Show what is requested, installed and pending. Changes nothing. |
| `nux install <pkg>[@req]...` | Add packages to the environment and sync. |
| `nux remove <pkg>...` | Take packages out of the environment and sync. |
| `nux clean` | Remove build leftovers. |
| `nux purge` | Remove versions no environment uses. |
| `nux reset` | Remove everything Nux installed, to start over. |
| `nux help [command]` | List commands, or explain one. |

Common options:

| Option | Meaning | Commands |
| --- | --- | --- |
| `-e <path>` | Use this environment. | sync, status, install, remove |
| `-g` | Use the global environment. | sync, status, install, remove |
| `-y` | Don't ask for confirmation. | everything that changes things |
| `--wait` | Wait for another Nux command to finish. | everything that changes things |
| `--dry-run` | Only show what would be removed. | clean, purge, reset |
| `--locked` | Fail instead of changing `Nux.lock`. | sync |
| `--sources`, `--cache` | Also remove sources, the download cache. | clean |
| `--keep-cache` | Keep the download cache. | reset |

## Version requirements

| Requirement | Matches |
| --- | --- |
| `latest` or `*` | Any version (the newest is chosen). |
| `1.2.3` | Exactly 1.2.3 (`1.2` equals `1.2.0`). |
| `^1.2.3` | Same major version, at least 1.2.3. For `^0.2.3`: 0.2.x only. |
| `~1.2.3` | Same minor version, at least 1.2.3. |
| `>=1.2`, `<2`, `>1`, `<=3` | Comparisons. |
| `>=1.2, <2` | All parts at once. |

When several versions match: the one in `Nux.lock`, else one the host
provides, else the newest.

## `Nux.toml`

```toml
[environment]
name = "myproject"            # default: the directory's name

[packages]
jq = "~1.7"

[[repository]]                # searched in order, before config.toml's
path = "./recipes"
```

## `~/.config/nux/config.toml`

```toml
[nux]
home = "~/.nux"
jobs = 4                      # recipes read at the same time
parallel_builds = 4           # packages built at the same time

[[repository]]
path = "~/recipes"

[sandbox]
mode = "auto"                 # "auto", "required", "off"
network = true
shared_caches = false
```

## Recipes

```nux
program name {
    let release = "1.2.3";
    version(release);
    source(url = "https://example.com/name-${release}.tar.gz");
    add_dependency(zlib, "^1.3");

    bash { tar -xzf $NUX_SOURCE_PATH --strip-components=1 }
    autotools(options = ["--disable-docs"]);

    set_bin("bin/name");
    export_lib("name");
}
```

### Declaring a version

| Function | Purpose |
| --- | --- |
| `version("1.2.3")` | Start a version. What follows describes it. |
| `source(url = "...")` | Download a file. It is in `$NUX_SOURCE_PATH`. |
| `source(git = "...", tag = "v1")` | Check out a git tag (or branch). |
| `source(local = "dir")` | Use files next to `Nux.toml`. |
| `add_dependency(name, "req")` | Need another package (requirement defaults to `latest`). |
| `bash { ... }` | Build commands, one per line. |
| `set_bin("bin/x")` | The program to link in `~/.nux/bin`, as `x`. |
| `set_bin("bin/x", name = "x2")` | The same, linked as `x2`. |
| `export_lib("x", lib = "lib", include = "include")` | A library for dependents. |
| `use_host("x")` | This version is the machine's `x`; nothing is built. |

`source(...)` also takes `proxy = "http://..."`.

### Build-system helpers

| Helper | Specific arguments |
| --- | --- |
| `autotools()` | `reconf = true` runs `autoreconf -fi` first. |
| `cmake()` | `build_type` (`"Release"`), `build_dir`, `generator`. |
| `meson()` | `build_type` (`"release"`), `build_dir`. |
| `make()` | `targets = [...]`, `install` (`""` skips it), `prefix_var` (`"PREFIX"`). |
| `cargo()` | `features = [...]`, `locked = true`. |
| `go()` | `packages = [...]` (`["."]`), `ldflags`. |

All accept `options = [...]`, `dir`, `prefix`, `jobs` and
`env = ["NAME=value"]`.

### Asking about the machine

| Expression | Gives |
| --- | --- |
| `host_version("x")` | The version `x --version` prints, or `null`. |
| `host_version("x", "-V")` | The same with another flag. |
| `host_bin("x")` | The path of `x` on the `PATH`, or `null`. |
| `exec("prog", ["arg"])` | `.stdout`, `.stderr`, `.status`, `.success`. |
| `nux.arch`, `nux.os` | `"x86_64"`, `"linux"`... |
| `trim(text)` | `text` without surrounding whitespace. |
| `print(value)` | Prints while the recipe is read. |

### Language

```nux
let x = "a";  x = "b";                    // variables
"value: ${x}"                             // interpolation (plain names only)
if x == "b" && !done { } else if y { } else { }
while i < 3 { i = i + 1; }
return;                                   // stop reading the recipe
// comment    /* comment */
```

Values: strings, numbers, `true`/`false`, lists `[...]`, `null`.
`false`, `0`, `""`, `[]` and `null` are false.

### In `bash` blocks and helpers

| Variable | Value |
| --- | --- |
| `$NUX_PREFIX` | Where to install this package. |
| `$NUX_SOURCE` | The source directory. |
| `$NUX_SOURCE_PATH` | The downloaded file (for `url` sources). |
| `$NUX_BUILD` | The build directory, where commands run. |
| `$NUX_JOBS` | The number of CPUs. |
| `$NUX_DEP_<NAME>` | A dependency's install directory (`NUX_DEP_ZLIB`). |
| `$CPPFLAGS`, `$LDFLAGS` | Dependencies' headers and libraries (with rpath). |

Dependencies' programs are on the `PATH`. CMake, Meson and `pkg-config` find
their libraries.

## Where things are

| Path | Holds |
| --- | --- |
| `~/.nux/bin` | Your installed commands. Put it on your `PATH`. |
| `~/.nux/packages/<name>/<version>` | A package's installation, source and `build.log`. |
| `~/.nux/cache` | Downloads and git repositories. |
| `~/.config/nux/config.toml` | Your settings. |
| `~/.config/nux/Nux.toml` | Your global environment. |
