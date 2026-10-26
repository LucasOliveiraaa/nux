# Nux

Nux builds software from source and installs it in your home directory, with
no root access and without touching your system's package manager.

You describe how to build each program in a small recipe (a `.nux` file),
list the programs you want in an environment (`Nux.toml`), and run
`nux sync`. Nux then:

- picks versions that match your requirements (`latest`, `^1.2`, `>=2, <3`...);
- downloads sources and builds them in dependency order, several at a time;
- reuses tools your machine already has, such as its compiler;
- builds in a sandbox that can only write the package's own directories;
- links the installed programs in `~/.nux/bin`;
- records exact versions and sources in `Nux.lock`, so that builds can be
  reproduced.

## Installation

You can install Nux with this installation script:

```sh
curl -fsSL https://github.com/LucasOliveiraaa/nux/releases/latest/download/install.sh | bash
```

If you prefer to build from source, you need Linux, Rust 1.85 or later, `git`, `curl` and a C compiler.

```sh
git clone https://github.com/LucasOliveiraaa/nux.git
cd nux
cargo build --release
cp target/release/nux ~/.local/bin/    # or anywhere on your PATH
```

## Usage

Create an environment and build it:

```sh
nux init hello-env
cd hello-env
nux sync
~/.nux/bin/hello                       # Hello from Nux!
```

`nux init` writes a `Nux.toml` that lists the packages you want, and a
`repository/` with their recipes:

```toml
[packages]
hello = "latest"

[[repository]]
path = "./repository"
```

Add `~/.nux/bin` to your `PATH` to run installed programs directly:

```sh
export PATH="$HOME/.nux/bin:$PATH"
```

Nux also works as a personal package manager. Once your recipe repositories
are listed in `~/.config/nux/config.toml`:

```sh
nux install jq          # build and install jq
nux remove jq           # uninstall it
nux status              # see what is installed
```

Run `nux help` for every command. To learn more, follow
[the lessons](docs/README.md), and try the ready-to-run environments in
[example/](example/README.md).

## License

Nux is released under the [MIT License](LICENSE). Copyright (c) 2026 Lucas de
Oliveira.
