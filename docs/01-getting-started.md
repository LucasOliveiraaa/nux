# Lesson 1: Getting started

In this lesson you install Nux, build your first package and run it. It
takes about ten minutes.

## Install Nux

Nux is built from its source with Cargo, Rust's build tool. From the
directory where you cloned Nux:

```sh
cargo build --release
```

This produces `target/release/nux`. Put it somewhere on your `PATH`, for
example:

```sh
cp target/release/nux ~/.local/bin/
nux --help
```

If `nux --help` prints a list of commands, Nux is installed.

## Create an environment

Nux works with *environments*. An environment is a directory with a
`Nux.toml` file that lists the packages you want. `nux init` creates one,
ready to try:

```sh
nux init hello-env
cd hello-env
```

```
Created environment hello-env in `hello-env`:
  Nux.toml
  repository/hello.nux
```

Look at what it created:

```toml
# Nux.toml
[environment]
name = "hello-env"

[packages]
hello = "latest"

[[repository]]
path = "./repository"
```

This environment asks for one package, `hello`, at its latest version.
Packages are described by *recipes*, `.nux` files that say how to build them.
Nux looks for recipes in the directories listed under `[[repository]]`, here
`./repository`, which holds `hello.nux`. You will write your own recipes in
lesson 4. For now, Nux only needs to know they are there.

## Build it

Ask Nux to make your environment real:

```sh
nux sync
```

```
Environment: hello-env

Resolving dependencies...
Checking installed packages...

The environment requests the following packages:
  hello 1.0.0

The following packages will be built:
  hello 1.0.0

1 package to build, 0 sources to download.

Continue? [Y/n] y

No Nux workspace found, creating one...

  Updated `Nux.lock`.

  Building hello... Done!

  Finished building 1 package in 0.0s

  Updating environment...
  Updating executable links...

Nux finished syncing the environment.
```

Nux always tells you what it is about to build and asks before starting.
Press Enter (or type `y`) to go ahead. To skip the question, for example in a
script, pass `-y`: `nux sync -y`.

The first time you run Nux, it takes a second longer to start. Later runs are
faster.

## Run it

Everything Nux builds goes in `~/.nux`, your *workspace*. The programs it
installs are linked in `~/.nux/bin`:

```sh
~/.nux/bin/hello
```

```
Hello from Nux!
```

## Add `~/.nux/bin` to your `PATH`

So that you can type `hello` instead of `~/.nux/bin/hello`, add this line to
your shell's startup file (`~/.bashrc`, `~/.zshrc`...):

```sh
export PATH="$HOME/.nux/bin:$PATH"
```

Open a new terminal, and:

```sh
hello
```

`nux sync` reminds you when `~/.nux/bin` is not on your `PATH` yet.

## Check where things stand

`nux status` shows what your environment asks for and what is installed. It
never changes anything:

```sh
nux status
```

```
Environment: hello-env (Nux.toml)
Workspace:   /home/you/.nux

Requested packages:
  hello latest -> 1.0.0 installed

Executable links:
  hello -> hello 1.0.0 linked

Lock file:   Nux.lock is up to date

Environment is up to date.
```

`hello latest -> 1.0.0` means: you asked for the latest version, and that is
1.0.0.

Run `nux sync` again. There is nothing left to do, so Nux says so right away:

```
Environment is already up to date.
```

Nux only builds what is missing or what changed. You can run `nux sync` as
often as you like.

## What about `Nux.lock`?

The first sync also wrote a `Nux.lock` file next to `Nux.toml`. It records
the exact versions Nux chose, so that the next sync, or a colleague's, picks
the same ones. Lesson 3 explains how to use it. For now, leave it there.

## What you learned

- An **environment** is a directory with a `Nux.toml` that lists packages.
- **Recipes** (`.nux` files in a repository) say how to build each package.
- `nux sync` builds what the environment needs, after asking you.
- Programs are installed in **`~/.nux`** and linked in `~/.nux/bin`.
- `nux status` shows where things stand without changing anything.

Next: [Installing packages](02-installing-packages.md), where you use Nux
like a package manager.
