# Lesson 2: Installing packages

In lesson 1, you built a package that came with its environment. Most of the
time, though, you want to install a tool you need, wherever you are, the way
you would with `apt` or `brew`. That is what this lesson is about.

## Where packages come from

Nux has no central package index. Packages come from *repositories*:
directories of recipes, which you can write yourself (from lesson 4), share
with your team, or take from someone else.

Nux's source comes with example recipes you can use right away, in
`example/advanced/repository` and `example/repository`. Among them: `jq`,
`ripgrep`, `lazygit`, `curl` and `zlib`.

## Tell Nux where your recipes are

Settings that apply everywhere live in `~/.config/nux/config.toml`. Create it
and list the two example repositories, replacing `/path/to/nux` with where
you cloned Nux:

```toml
# ~/.config/nux/config.toml
[[repository]]
path = "/path/to/nux/example/advanced/repository"

[[repository]]
path = "/path/to/nux/example/repository"
```

When two repositories have a recipe for the same package, the first one
listed wins.

## Install a package

Go to any directory that has no `Nux.toml`, your home for example, and
install `jq`, a command-line JSON processor:

```sh
cd ~
nux install jq
```

```
Environment: global (global)

Creating `/home/you/.config/nux/Nux.toml`, requesting:
  jq latest

Resolving dependencies...
Checking installed packages...

The environment requests the following packages:
  jq 1.7.1

The following packages are provided by the host:
  gcc 14.2.0 /usr/bin/gcc

The following packages will be built:
  jq 1.7.1

1 package to build, 1 source to download.

Continue? [Y/n] y
...
  Downloading jq  [==============================] 100% 1.4 MB/s
...
  Building gcc... Provided by the host
  Building jq...  Done!

  Finished building 1 package in 1m 2s
...
Nux finished syncing the environment.
```

It takes a minute or so: Nux downloads jq's source code and compiles it.

```sh
jq --version
```

```
jq-1.7.1
```

A few things happened:

- **You used the global environment.** Outside of a project, `nux install`
  adds packages to your personal environment in `~/.config/nux/Nux.toml`,
  and creates it the first time.
- **Nux used your compiler.** jq needs `gcc` to build. Your machine already
  has one, so Nux used it instead of building a compiler ("provided by the
  host"). Lesson 7 shows how recipes do this.
- **`Nux.toml` changed only once you said yes.** If you answer `n`, or if a
  package doesn't exist, nothing is written.

## Choose a version

`jq` alone means "the latest version". To ask for a specific version or a
range, add `@` and a requirement:

```sh
nux install jq@1.7.1        # exactly 1.7.1
nux install jq@~1.7         # 1.7.x, the newest available
nux install ripgrep@^14     # 14.x.y, the newest available
```

The common requirements are:

| You write | You get |
| --- | --- |
| `latest` | The newest version. |
| `1.7.1` | Exactly 1.7.1. |
| `^14.1` | Any 14.x at or above 14.1: same major version. |
| `~1.7` | Any 1.7.x: same minor version. |
| `>=1.6, <2` | Anything that satisfies every part. |

If a package is already installed, `nux install` with a new requirement
replaces the old requirement and syncs again.

## See what you have

Outside of a project, `nux status` shows your global environment. Anywhere,
`-g` asks for it explicitly:

```sh
nux status -g
```

```
Environment: global (/home/you/.config/nux/Nux.toml, global)
Workspace:   /home/you/.nux

Requested packages:
  jq latest -> 1.7.1 installed

Dependencies:
  gcc 14.2.0 provided by the host (/usr/bin/gcc)

Executable links:
  jq -> jq 1.7.1 linked

Lock file:   Nux.lock is up to date

Environment is up to date.
```

## Remove a package

```sh
nux remove jq
```

```
Removing from `/home/you/.config/nux/Nux.toml`:
  jq latest
...
  Removed link: jq

Nux finished syncing the environment.

jq 1.7.1 is no longer used by any environment; run `nux purge` to remove it.
```

The `jq` command is gone, but the build is kept on disk: if you install jq
again, Nux reuses it instantly. When you want the space back:

```sh
nux purge
```

```
Keeping what these environments use: global, hello-env

The following will be removed:
  unused package jq 1.7.1 12.9 MB

1 item, 12.9 MB.

Continue? [Y/n] y

  Removed 1 item, freed 12.9 MB.
```

`purge` removes only what no environment uses. Here it keeps `hello`, which
your `hello-env` environment from lesson 1 still uses.

## Install several at once

```sh
nux install ripgrep lazygit
```

Nux builds independent packages at the same time, up to four by default.
ripgrep needs Rust (`cargo`) and lazygit needs Go. Nux uses yours if they are
installed, and otherwise downloads them.

## What you learned

- Packages come from **repositories** of recipes, which you list in
  `~/.config/nux/config.toml`.
- **`nux install <package>`** outside of a project adds it to your **global
  environment** and builds it.
- **`name@requirement`** picks a version: `jq@~1.7`, `ripgrep@^14`.
- **`nux remove`** takes a package out; **`nux purge`** frees the space of
  what nothing uses.
- **`-g`** works on the global environment from anywhere.

Next: [Project environments](03-project-environments.md), to give each
project its own tools.
