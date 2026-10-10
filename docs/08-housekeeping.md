# Lesson 8: Housekeeping

Builds take disk space. In this lesson you learn what Nux keeps, how to
reclaim space safely, and how to start over.

## What Nux keeps, and why

Everything lives in `~/.nux`:

- **`bin/`** holds the commands you installed (links into `packages/`).
- **`packages/`** holds every version of every package Nux built, with its
  build log and its downloaded source.
- **`cache/`** holds downloads and git repositories, shared by all versions.
  Thanks to it, reinstalling something Nux built before needs no network.

Nux keeps builds around on purpose: switching versions, or reinstalling a
package you removed, is then instant. Three commands give space back, from
the gentlest to the most thorough.

## `nux clean`: leftovers

```sh
nux clean
```

```
The following will be removed:
  build log cjson 1.7.18  5.8 kB
  build log jq 1.7.1      65.7 kB
  build log lua 5.3.6     6.9 kB
  ...

9 items, 94.3 kB.
Installed packages and downloaded sources are kept; pass `--sources` to remove sources too.
Installed packages no environment uses are removed by `nux purge`.

Continue? [Y/n]
```

It removes what builds leave behind: logs, half-finished downloads, the build
directories of failed builds. Everything installed keeps working. Two options
go further:

- **`--sources`** also removes downloaded sources. Rebuilding would then take
  them from the cache, or download them again.
- **`--cache`** also empties the download cache.

## `nux purge`: what no environment uses

When you remove a package, or change a requirement, the old version stays
installed. `nux purge` removes the versions no environment uses any more:

```sh
nux purge
```

```
Keeping what these environments use: global, hello-env, myproject, tools

The following will be removed:
  unused package lua 5.4.7 1.5 MB

1 item, 1.5 MB.
```

It knows what each environment uses from that environment's last
`nux sync`. Two consequences:

- **Sync after editing a `Nux.toml` by hand**, so that `purge` sees the
  change.
- **Deleting a project is enough.** Once a project's `Nux.toml` is gone,
  `purge` removes what only it used.

## `nux reset`: start over

To wipe the workspace entirely:

```sh
nux reset
```

```
The following will be removed:
  executable links    bin      242 B
  packages            packages 19.2 MB
  environment records state    1.6 kB
  download cache      cache    4.6 MB

4 items, 23.8 MB.
Every installed package and its links will be gone. Environments, their Nux.lock and the configuration are kept.
```

Your environments, lock files and `config.toml` are not touched, so
`nux sync` rebuilds everything as it was. Add `--keep-cache` to keep
downloads, so that rebuilding doesn't need the network. Files you put in
`~/.nux` yourself are left alone.

## Look before you leap

`clean`, `purge` and `reset` always show what they will remove and ask first.
To only look, add `--dry-run`:

```sh
nux purge --dry-run
```

To skip the question in a script, add `-y`.

## What you learned

- Nux keeps builds, sources and downloads so that switching and reinstalling
  are fast.
- **`nux clean`** removes leftovers. `--sources` and `--cache` go further.
- **`nux purge`** removes versions no environment uses, based on each
  environment's last sync.
- **`nux reset`** wipes the workspace. Environments stay, and `nux sync`
  rebuilds.
- **`--dry-run`** shows what would happen.

Next: [Configuration and the sandbox](09-configuration-and-sandbox.md).
