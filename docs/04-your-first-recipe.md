# Lesson 4: Your first recipe

So far you have used recipes others wrote. Now you write one. You will
package a small C program that lives next to your environment, so there is
nothing to download.

## Set up

Create an environment with its own recipe repository:

```sh
mkdir -p tools/repository tools/src/greet
cd tools
```

```toml
# Nux.toml
[environment]
name = "tools"

[packages]
greet = "latest"

[[repository]]
path = "./repository"
```

And the program, in `src/greet/greet.c`:

```c
#include <stdio.h>

int main(int argc, char **argv) {
    printf("Hello, %s!\n", argc > 1 ? argv[1] : "world");
    return 0;
}
```

## Write the recipe

Create `repository/greet.nux`:

```nux
program greet {
    version("1.0.0");
    source(local = "src/greet");

    bash {
        mkdir -p $NUX_PREFIX/bin
        cc greet.c -o $NUX_PREFIX/bin/greet
    }

    set_bin("bin/greet");
}
```

Line by line:

- **`program greet { ... }`** names the package. That is the name you use in
  `[packages]` and `nux install`. Names are letters, digits and `_`.
- **`version("1.0.0")`** declares a version. Everything after it describes
  that version.
- **`source(local = "src/greet")`** says where the source is: here, a
  directory relative to `Nux.toml`.
- **`bash { ... }`** holds the build commands. Nux runs them in a copy of the
  source, so `greet.c` is right there.
- **`$NUX_PREFIX`** is where the package must install its files. Each version
  of each package has its own.
- **`set_bin("bin/greet")`** tells Nux which file is the program, relative to
  `$NUX_PREFIX`. Nux links it as `~/.nux/bin/greet`.

## Build and run it

```sh
nux sync
```

```
The following packages will be built:
  greet 1.0.0
...
  Building greet... Done!
...
Nux finished syncing the environment.
```

```sh
greet
greet Nux
```

```
Hello, world!
Hello, Nux!
```

## How the `bash` block runs

A few rules worth knowing before you write longer builds:

- **One line, one command.** Each line runs in its own shell, so `cd` or
  `export` on one line has no effect on the next. Chain them instead:
  `cd src && make`.
- **Long lines** can continue with a trailing `\`.
- **The first failing command stops the build.**
- **Nux variables** written `${name}` are replaced by their value (more in
  lesson 5). Anything else, like `$NUX_PREFIX` or `$HOME`, is left for the
  shell.
- **Comments** start with `//` outside `bash` blocks, and `#` inside them.

## When a build fails

Break the build on purpose, by linking a library that doesn't exist:

```nux
cc greet.c -lnothere -o $NUX_PREFIX/bin/greet
```

Recipes are read every time Nux runs, so after saving the file, `nux sync`
notices the change and rebuilds:

```
  Building greet... Failed

error: failed to build greet 1.0.0: `cc greet.c -lnothere -o $NUX_PREFIX/bin/greet` exited with exit status: 1
$ mkdir -p $NUX_PREFIX/bin
$ cc greet.c -lnothere -o $NUX_PREFIX/bin/greet
/usr/bin/ld: cannot find -lnothere: No such file or directory
collect2: error: ld returned 1 exit status
full log: /home/you/.nux/packages/greet/1.0.0/build.log
```

Nux shows the command that failed and the end of its output. The full log
has everything the build printed. Fix the recipe and run `nux sync` again:
Nux retries only what failed.

While a package is broken like this, its command doesn't work either. Fix the
recipe, sync again, and it is back.

## When the recipe itself is wrong

Mistakes in the recipe are caught before anything is built, and point at the
exact place:

```nux
version(1.0);
```

```
error: argument `version` of `version` must be a string, found number
 --> ./repository/greet.nux:2:13
  |
2 |     version(1.0);
  |             ^^^
```

## When Nux rebuilds

Nux rebuilds a package when its recipe changes in a way that matters: its
commands, source, version, dependencies or `set_bin`. `//` comments and
blank lines don't count. A `#` line inside a `bash` block is part of the
build, so changing it does trigger a rebuild.

Nux does **not** watch the files of a `local` source. After editing
`greet.c`, bump the version so Nux builds again:

```nux
version("1.0.1");
```

The finished environment is in `example/hello`, if you want to compare.

## What you learned

- A recipe is a **`program`** with one or more **`version`**s.
- **`source(local = ...)`** builds from files next to your environment.
- **`bash { ... }`** runs build commands. Install into **`$NUX_PREFIX`**.
- **`set_bin`** says which file is the program, so Nux links it.
- Failed builds show the failing command and point at **`build.log`**.
- Changing the recipe triggers a rebuild. To rebuild after editing a
  `local` source, **bump the version**.

Next: [Packaging real software](05-packaging-real-software.md), with
downloads and build systems.
