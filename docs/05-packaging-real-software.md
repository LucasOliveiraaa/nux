# Lesson 5: Packaging real software

Real software is downloaded, comes in archives or git repositories, and
builds with `make`, CMake, Cargo and friends. In this lesson you package Lua,
a small programming language, in two versions.

Keep working in the `tools` environment from lesson 4.

## Download a release

Lua releases are `.tar.gz` archives on lua.org. Create
`repository/lua.nux`:

```nux
program lua {
    version("5.4.7");
    source(url = "https://www.lua.org/ftp/lua-5.4.7.tar.gz");

    bash { tar -xzf $NUX_SOURCE_PATH --strip-components=1 }
    make(targets = ["all"], prefix_var = "INSTALL_TOP");

    set_bin("bin/lua");
}
```

What's new:

- **`source(url = ...)`** downloads one file. Nux gives you its path in
  `$NUX_SOURCE_PATH`, and leaves unpacking it to you: the `tar` line
  extracts the archive into the build directory.
- **`make(...)`** is a *build-system helper*. Instead of writing the
  `make` and `make install` commands yourself, you describe what is special
  about this project, and Nux writes the commands:

  ```
  make -C . -j $NUX_JOBS "INSTALL_TOP=$NUX_PREFIX" all
  make -C . "INSTALL_TOP=$NUX_PREFIX" install
  ```

  Lua's Makefile calls its install directory `INSTALL_TOP` rather than the
  usual `PREFIX`, hence `prefix_var`.

Add it to your environment and build:

```sh
nux install lua
lua -v
```

```
Lua 5.4.7  Copyright (C) 1994-2024 Lua.org, PUC-Rio
```

Downloads go to a cache, so a source is never downloaded twice. Some servers
are slow: lua.org can take a minute.

## Build-system helpers

Each helper writes the usual configure, build and install steps for one
build system:

| Helper | For projects built with |
| --- | --- |
| `autotools()` | `./configure && make && make install` |
| `cmake()` | CMake |
| `meson()` | Meson and Ninja |
| `make()` | A plain Makefile |
| `cargo()` | Rust's Cargo |
| `go()` | Go modules |

They all take a few common arguments, all optional:

```nux
cmake(options = ["-DBUILD_TESTING=OFF"]);           // extra arguments
autotools(options = ["--disable-docs"], env = ["CFLAGS=-O2"]);
make(dir = "src");                                   // project in a subdirectory
```

And some of their own: `build_type` for CMake and Meson, `features` for
Cargo, `targets` for Make, and so on. The [cheat sheet](cheatsheet.md) lists
them all.

Every helper builds with `$NUX_JOBS` parallel jobs: Nux sets it to the number
of CPUs.

When no helper fits, write the commands in a `bash` block. You can mix both:
a `bash` block runs where it appears, before or after a helper's commands.

## Download from git

For a git repository, give its URL and, usually, a tag:

```nux
source(git = "https://github.com/DaveGamble/cJSON.git", tag = "v1.7.18");
```

Nux checks out that tag (only the files, without history), and the build runs
in a copy of it. You will use this recipe in lesson 6.

## Offer several versions

A recipe can describe several versions of its program. Each `version(...)`
starts a fresh description. Let's add Lua 5.3, which builds a little
differently:

```nux
program lua {
    let release = "5.4.7";
    version(release);
    source(url = "https://www.lua.org/ftp/lua-${release}.tar.gz");
    bash { tar -xzf $NUX_SOURCE_PATH --strip-components=1 }
    make(targets = ["all"], prefix_var = "INSTALL_TOP");
    set_bin("bin/lua");

    release = "5.3.6";
    version(release);
    source(url = "https://www.lua.org/ftp/lua-${release}.tar.gz");
    bash { tar -xzf $NUX_SOURCE_PATH --strip-components=1 }
    make(targets = ["posix"], prefix_var = "INSTALL_TOP");
    set_bin("bin/lua");
}
```

- **`let release = "5.4.7";`** creates a variable. **`release = "5.3.6";`**
  changes it.
- **`"lua-${release}.tar.gz"`** puts the variable's value in the string. In a
  `bash` block too, `${release}` would be replaced.

Environments choose a version with their requirement:

```sh
nux install lua@~5.3
lua -v
```

```
Lua 5.3.6  Copyright (C) 1994-2020 Lua.org, PUC-Rio
```

```sh
nux status
```

```
Requested packages:
  greet latest -> 1.0.0 installed
  lua   ~5.3 ->   5.3.6 installed (also installed: 5.4.7)
```

5.4.7 stays installed next to 5.3.6. Another environment can still use it,
and `nux purge` removes it when none does.

## A few language basics

Recipes are small programs, so you can use conditions and loops when a build
differs between versions or machines:

```nux
let release = "5.4.7";
if release == "5.4.7" {
    make(targets = ["all"], prefix_var = "INSTALL_TOP");
} else {
    make(targets = ["posix"], prefix_var = "INSTALL_TOP");
}
```

Values are strings (`"5.4.7"`), numbers (`3`), booleans (`true`), lists
(`["a", "b"]`) and `null`. Comparisons use `==`, `!=`, `<`, `>`, and
conditions combine with `&&`, `||` and `!`. `while` loops exist too. To see a
value while you write a recipe, `print(release);` prints it when Nux reads
the recipe.

## What you learned

- **`source(url = ...)`** downloads a file into `$NUX_SOURCE_PATH`. You
  unpack it in the build.
- **`source(git = ..., tag = ...)`** checks out a git tag.
- **Helpers** like `make()`, `cmake()`, `autotools()`, `meson()`, `cargo()`
  and `go()` write the standard build steps for you.
- One recipe can offer **several versions**. Requirements choose between
  them.
- **`let`** variables and **`${name}`** keep recipes short.

Next: [Dependencies and libraries](06-dependencies-and-libraries.md).
