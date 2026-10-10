# Lesson 7: Using what your machine has

Building a compiler or Python from source takes a long time, and your
machine probably has them already. In this lesson you write recipes that use
the installed version when there is one, and build only when there isn't.

Keep working in the `tools` environment.

## Use the host's program

Create `repository/python.nux`:

```nux
program python {
    // Use the machine's Python 3 when there is one.
    let host = host_version("python3");
    if host {
        version(host);
        use_host("python3");
        return;
    }

    // Otherwise, build it.
    let release = "3.12.7";
    version(release);
    source(url = "https://www.python.org/ftp/python/${release}/Python-${release}.tgz");
    bash { tar -xzf $NUX_SOURCE_PATH --strip-components=1 }
    autotools();
    set_bin("bin/python3");
}
```

- **`host_version("python3")`** runs `python3 --version` and returns the
  version it prints, like `"3.13.5"`. If there is no `python3`, it returns
  `null`, which counts as false in an `if`.
- **`version(host)`** declares a version numbered like the host's.
- **`use_host("python3")`** says this version *is* the host's `python3`.
  Nux never downloads, builds or links it.
- **`return`** stops reading the recipe, so the build instructions below are
  only used on machines without Python.

When several versions match a requirement, Nux prefers one the host
provides, even over a newer one it would have to build.

## Depend on it

Packages depend on it like on anything else. Create
`repository/sysinfo.nux`:

```nux
program sysinfo {
    version("1.0.0");
    add_dependency(python, "^3");

    bash {
        mkdir -p $NUX_PREFIX/bin
        python3 -c 'print("python works at build time")'
        printf '#!/bin/sh\necho "Hello from sysinfo"\n' > $NUX_PREFIX/bin/sysinfo
        chmod +x $NUX_PREFIX/bin/sysinfo
    }
    set_bin("bin/sysinfo");
}
```

```sh
nux install sysinfo
```

```
The following packages are provided by the host:
  python 3.13.5 /usr/bin/python3
...
  Building python...  Provided by the host
  Building sysinfo... Done!
```

On a machine without Python, the same recipe would build Python 3.12.7
first.

## Offer the host's version *and* a built one

With `return`, a machine that has Python only offers the host's version. A
package that needs exactly 3.12.7 then fails on a machine with 3.13:

```
error: in dependencies of `mytool` 1: no version of `python` satisfies `3.12.7` (available: 3.13.5)
```

To offer both, leave out the `return`, and declare the built version unless
the host already has it:

```nux
program python {
    let host = host_version("python3");
    if host {
        version(host);
        use_host("python3");
    }

    let release = "3.12.7";
    if host != release {
        version(release);
        source(url = "https://www.python.org/ftp/python/${release}/Python-${release}.tgz");
        bash { tar -xzf $NUX_SOURCE_PATH --strip-components=1 }
        autotools();
        set_bin("bin/python3");
    }
}
```

Now `add_dependency(python, "^3")` still uses the host's Python, because Nux
prefers it. `add_dependency(python, "3.12.7")` builds 3.12.7.

Mind the command, though. A Python that Nux builds is linked as
`~/.nux/bin/python3`, and `~/.nux/bin` usually comes first on your `PATH`.
After that sync, typing `python3` runs 3.12.7, not your system's. Host
versions are never linked, so they never shadow anything.

## Ask the machine questions

Sometimes a build depends on the machine: its architecture, its kernel, or
whether a library is installed. Recipes can find out while Nux reads them.

**`nux.arch` and `nux.os`** describe the machine (`"x86_64"`, `"aarch64"`;
`"linux"`). Use them to pick a download:

```nux
let arch = nux.arch;
source(url = "https://example.com/tool-${arch}.tar.gz");
```

`${...}` only accepts a plain variable name, so copy `nux.arch` into a
variable first. `"${nux.arch}"` would not be replaced.

**`exec(program, arguments)`** runs a program and gives you what it printed:

```nux
let kernel = trim(exec("uname", ["-r"]).stdout);     // "6.12.111+deb13-amd64"

let ssl = exec("pkg-config", ["--exists", "openssl"]);
if !ssl.success {
    add_dependency(openssl);     // only when the machine lacks it
}
```

The result has `stdout`, `stderr`, `status` (the exit code) and `success`
(`true` when the exit code is 0). `trim` removes the newline at the end.

**`host_bin("name")`** gives the path of a program on your `PATH`, or `null`.

Recipes are read every time Nux runs, `nux status` included. So use `exec`
only to *ask* questions, and keep them quick. Building belongs in `bash`
blocks and helpers.

## What you learned

- **`host_version`** finds an installed program's version. **`use_host`**
  makes a recipe version stand for it.
- The **`if host { ...; return; }`** pattern uses the host when possible and
  builds otherwise. Leave out `return` to offer a built version as well.
- Nux **prefers host versions** when they satisfy a requirement.
- **`nux.arch`**, **`nux.os`**, **`exec`** and **`host_bin`** let recipes
  adapt to the machine. Copy values into a variable before using them in
  `${...}`.

Next: [Housekeeping](08-housekeeping.md).
