# Examples

Ready-to-run environments, one for each part of [the lessons](../docs/README.md).
Run them from the root of the Nux repository:

```sh
nux status -e example/hello     # what would be built
nux sync -e example/hello       # build it
```

| Environment | Shows | Lesson |
| --- | --- | --- |
| [hello](hello/Nux.toml) | A recipe for a program next to the environment. Nothing to download, builds offline in a second. | [4](../docs/04-your-first-recipe.md) |
| [cli-tools](cli-tools/Nux.toml) | Everyday tools (`jq`, `ripgrep`) from shared recipes, with version requirements. | [2](../docs/02-installing-packages.md), [3](../docs/03-project-environments.md) |
| [lua/current](lua/current/Nux.toml), [lua/legacy](lua/legacy/Nux.toml) | Two environments needing different Lua releases. Each release is linked under its own name (`lua5.4`, `lua5.3`). | [5](../docs/05-packaging-real-software.md) |
| [json-tools](json-tools/Nux.toml) | A program linked against a library (cJSON), which also runs Lua while it builds. | [6](../docs/06-dependencies-and-libraries.md) |
| [host-tools](host-tools/Nux.toml) | Reusing the machine's Python, and recipes that query the machine. | [7](../docs/07-using-the-host.md) |

## The shared recipes

[recipes/](recipes) is a recipe repository the environments share. You can
also use it for your own environments, by listing it in
`~/.config/nux/config.toml` (lesson 2):

| Recipe | Notes |
| --- | --- |
| [gcc](recipes/gcc.nux), [cmake](recipes/cmake.nux) | Your system's when you have them; otherwise built from source. |
| [rust](recipes/rust.nux) | Your `cargo` when you have one; otherwise the official prebuilt release for your machine. |
| [python](recipes/python.nux) | Your system's Python 3, and 3.12 built from source for packages that ask for it, linked as `python3.12`. |
| [jq](recipes/jq.nux) | Built with `autotools()`. |
| [lua](recipes/lua.nux) | Two release series, each linked under its own name. |
| [cjson](recipes/cjson.nux) | A library from a git tag, built with `cmake()`. |
| [zlib](recipes/zlib.nux) | A library only, built with `autotools()`. |
| [ripgrep](recipes/ripgrep.nux) | Built with `cargo()`; the command is `rg`. |

None of the environments come with a `Nux.lock`. Their versions depend on
your machine (which compiler and Python it has), so the first `nux sync`
writes a lock that fits it.
