# Lesson 6: Dependencies and libraries

Most programs need other programs to build, and libraries to link against.
In this lesson you package a library, cJSON, and a program that uses both it
and Lua.

Keep working in the `tools` environment.

## Package a library

cJSON is a small JSON library built with CMake. Create
`repository/cjson.nux`:

```nux
program cjson {
    version("1.7.18");
    source(git = "https://github.com/DaveGamble/cJSON.git", tag = "v1.7.18");
    cmake(options = ["-DENABLE_CJSON_TEST=OFF", "-DCMAKE_INSTALL_LIBDIR=lib"]);
    export_lib("cjson");
}
```

- **`cmake(...)`** configures, builds and installs with CMake. It uses the
  `cmake` on your `PATH`.
- **`export_lib("cjson")`** says the package provides a library, `libcjson`,
  with its headers in `include/` and the library in `lib/`. Nux checks they
  are there after the build, and makes them available to every package that
  depends on this one.
- **`-DCMAKE_INSTALL_LIBDIR=lib`** makes CMake install into `lib`. Some
  projects default to `lib64` on some systems. Either force `lib` like this,
  or tell Nux where to look: `export_lib(name = "cjson", lib = "lib64")`.

A library has no command, so there is no `set_bin`.

## Depend on it

Now a program that pretty-prints JSON with cJSON. It also uses Lua while it
builds, to generate a header. Put the C file in `src/pretty/pretty.c`:

```c
#include <stdio.h>
#include <stdlib.h>
#include <cjson/cJSON.h>
#include "banner.h"

/* Reads JSON on standard input and prints it indented. */
int main(void) {
    char buffer[65536];
    size_t length = fread(buffer, 1, sizeof buffer - 1, stdin);
    buffer[length] = '\0';
    cJSON *json = cJSON_Parse(buffer);
    if (!json) {
        fprintf(stderr, "%s: not valid JSON\n", BANNER);
        return 1;
    }
    char *text = cJSON_Print(json);
    puts(text);
    free(text);
    cJSON_Delete(json);
    return 0;
}
```

And the recipe, `repository/pretty.nux`:

```nux
program pretty {
    version("1.0.0");
    source(local = "src/pretty");
    add_dependency(cjson, "^1.7");
    add_dependency(lua, "^5.3");

    bash {
        lua -e 'print(string.format("#define BANNER %q", "pretty " .. _VERSION))' > banner.h
        mkdir -p $NUX_PREFIX/bin
        cc pretty.c $CPPFLAGS $LDFLAGS -lcjson -o $NUX_PREFIX/bin/pretty
    }

    set_bin("bin/pretty");
}
```

**`add_dependency(name, requirement)`** declares what `pretty` needs. The
requirement is optional and defaults to `latest`.

```sh
nux install pretty
```

```
  Downloading cjson  [==============================] 100%
...
  Building lua...    Already built
  Building cjson...  Done!
  Building pretty... Done!
```

Nux builds dependencies first: cJSON, then `pretty`. Lua was already built.
Try it:

```sh
echo '{"name":"nux","tags":["build","tools"]}' | pretty
```

```
{
	"name":	"nux",
	"tags":	["build", "tools"]
}
```

## What a build sees of its dependencies

While `pretty` builds, Nux sets up its environment so that its dependencies
are easy to use:

- **Their programs are on the `PATH`.** That is why `lua` just works in the
  `bash` block.
- **Their libraries are found by the compiler.** `CPATH` and `CPPFLAGS` point
  at their headers. `LIBRARY_PATH` and `LDFLAGS` point at their libraries.
- **Build tools find them too.** CMake's `find_package` (through
  `CMAKE_PREFIX_PATH`), `pkg-config` (through `PKG_CONFIG_PATH`), and
  therefore Meson and most `configure` scripts.
- **`$NUX_DEP_<NAME>`** is each dependency's install directory, like
  `$NUX_DEP_CJSON`, for the occasional build that needs an explicit path:

  ```nux
  cmake(options = ["-DCJSON_ROOT=$NUX_DEP_CJSON"]);
  ```

This includes dependencies of dependencies.

When you write compiler commands yourself, pass `$LDFLAGS`. It also sets an
*rpath*, so the installed program finds the library on its own, with no
`LD_LIBRARY_PATH`:

```sh
ldd ~/.nux/bin/pretty | grep cjson
```

```
	libcjson.so.1 => /home/you/.nux/packages/cjson/1.7.18/install/lib/libcjson.so.1
```

## Different versions for different needs

Each requirement is resolved on its own. If one package needs `zlib ^1.3` and
another `zlib 1.2.13`, Nux builds both versions, and each package links
against the one it asked for.

## Rebuilds follow dependencies

When you change a dependency's recipe, Nux rebuilds it and everything that
depends on it. Change `cjson.nux` (add an option, say) and run `nux status`:
both `cjson` and `pretty` show `not built`.

The finished environment is in `example/json-tools`.

## What you learned

- **`export_lib("name")`** publishes a library to dependents. Use `lib = ...`
  when it isn't installed in `lib/`.
- **`add_dependency(name, requirement)`** declares a dependency. Nux builds
  it first.
- Dependencies' **programs are on the `PATH`** and their **libraries are
  found automatically** by compilers, CMake, Meson and `pkg-config`.
- Pass **`$LDFLAGS`** to hand-written compiler commands for a working rpath.
- **`$NUX_DEP_<NAME>`** gives a dependency's install directory.

Next: [Using what your machine has](07-using-the-host.md), to avoid building
what is already installed.
