# Lesson 3: Project environments

Your global environment is for tools you use everywhere. A project often
needs its own: a specific version of a library, a code generator, a
compiler. In this lesson you give a project its own environment, pin its
versions, and share it with others.

## Create a project environment

In a project directory, write a `Nux.toml`:

```sh
mkdir myproject && cd myproject
```

```toml
# Nux.toml
[environment]
name = "myproject"

[packages]
jq = "~1.7"
zlib = "^1.3"
```

The `[packages]` section maps each package to a
[version requirement](02-installing-packages.md#choose-a-version), the same
ones you used with `nux install`.

There is no `[[repository]]` here. This environment uses the repositories
from your `config.toml` (lesson 2). An environment can also list its own,
which it searches first:

```toml
[[repository]]
path = "./recipes"     # relative to Nux.toml
```

## Check before you build

```sh
nux status
```

```
Environment: myproject (Nux.toml)
Workspace:   /home/you/.nux

Requested packages:
  jq   ~1.7 -> 1.7.1 not built, source not downloaded
  zlib ^1.3 -> 1.3.1 not built, source not downloaded

Dependencies:
  gcc 14.2.0 provided by the host (/usr/bin/gcc)

Executable links:
  jq -> jq 1.7.1 not linked

Lock file:   Nux.lock needs updating: it does not exist yet

Environment is not synchronized: 2 packages to build, 2 sources to download, 1 link to update, Nux.lock to update.
Run `nux sync` to update it.
```

`status` shows the version each requirement selects (`^1.3 -> 1.3.1`), and
everything `sync` would do. Use it whenever you are unsure.

## Build it

```sh
nux sync
```

zlib is a library: it has no command to run, but programs you build later
can use it (lesson 6).

## Which environment is used?

Commands like `sync`, `status`, `install` and `remove` pick an environment
this way:

| You run | Environment used |
| --- | --- |
| `nux sync` in a directory with a `Nux.toml` | That one. |
| `nux sync` in a directory without one | Your global environment. |
| `nux sync -e path/to/project` | The one in that directory (or that file). |
| `nux sync -g` | Your global environment, wherever you are. |

So inside `myproject`, `nux install jq@1.7.1` edits `myproject/Nux.toml`,
not your global one.

Give each environment its own `name`. Nux uses names to keep track of what
each environment uses.

## The lock file

After the first sync, `myproject` has a `Nux.lock`:

```toml
[[package]]
name = "jq"
version = "1.7.1"
requested = true
source = "url+https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-1.7.1.tar.gz"
checksum = "sha256:478c9ca129fd2e3443fe27314b455e211e0d8c60bc8ff7df703873deeee580c2"
...
```

It records exactly what was used: each version, and a fingerprint of each
source (a checksum for downloads, a commit for git repositories). From then
on:

- **Versions stay put.** If a newer jq 1.7.2 appears in the recipes, `~1.7`
  would allow it, but Nux keeps 1.7.1 because the lock says so.
- **Sources are verified.** If a download no longer matches its checksum, Nux
  refuses it instead of building something different.

## Share the project

Commit three things:

- `Nux.toml`;
- `Nux.lock`;
- the project's own recipe directory, if it has one.

A teammate then runs `nux sync` in the project and gets the same versions,
built from the same sources.

In scripts and CI, use `--locked`. It builds exactly what the lock says, and
fails instead of changing anything:

```sh
nux sync --locked -y
```

If someone edited `Nux.toml` without updating the lock, `--locked` says so:

```
error: `Nux.lock` is out of date: `zlib` would change from 1.3.1 to 1.2.13; run `nux sync` without `--locked` to update it
```

## Update versions

To move to another version, change the requirement and sync:

```sh
nux install zlib@1.2.13     # or edit Nux.toml, then: nux sync
```

To get the newest versions your requirements allow, delete the lock and sync.
Nux picks the newest matching versions and writes a new lock:

```sh
rm Nux.lock
nux sync
```

Then commit the new `Nux.lock`.

## Several versions side by side

Two environments can use different versions of the same package. Nux
installs each version once, in its own place, and they never conflict. If
`myproject` uses zlib 1.2.13 while another project uses 1.3.1, both stay
installed.

Commands (things in `~/.nux/bin`) are shared, though: there is one `jq`
command. The last environment you synced decides which version it runs.

A ready-made project environment is in `example/cli-tools`: try
`nux status -e example/cli-tools` from the Nux repository.

## What you learned

- A project's **`Nux.toml`** lists the packages it needs, with version
  requirements.
- Nux uses `./Nux.toml` when there is one, your global environment
  otherwise, or what **`-e`** and **`-g`** say.
- **`Nux.lock`** pins exact versions and sources. Commit it with
  `Nux.toml`.
- **`nux sync --locked -y`** reproduces the lock exactly, for CI.
- To update: change requirements, or delete `Nux.lock` and sync.

Next: [Your first recipe](04-your-first-recipe.md), to package software
yourself.
