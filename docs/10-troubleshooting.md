# Lesson 10: Troubleshooting

Things go wrong: a build fails, a package can't be found, two processes get
in each other's way. Each section below starts with what you see, then
explains what it means and what to do.

When in doubt, start with `nux status`. It shows what Nux would do, without
doing it.

## A build failed

```
  Building greet... Failed

error: failed to build greet 1.0.0: `cc greet.c -lnothere -o $NUX_PREFIX/bin/greet` exited with exit status: 1
...
full log: /home/you/.nux/packages/greet/1.0.0/build.log
```

The message shows the failing command and the last lines it printed. Open
the full log for everything. Common causes:

- **A tool or library is missing on your machine.** Install it with your
  system's package manager, or add a recipe for it and an `add_dependency`.
- **The recipe is wrong.** Fix it and run `nux sync` again. Nux retries only
  what failed, and keeps what built successfully.
- **The sandbox blocked something.** See below.

While a package fails to build, its command doesn't work. Fixing the build
brings it back.

## `Read-only file system` during a build

A build tried to write outside its own directories, often to install into
`/usr/local` or to a fixed path. Builds may only write `$NUX_PREFIX` and
their build directory:

- Point the build at `$NUX_PREFIX`: `--prefix=$NUX_PREFIX`, `PREFIX=...`, or
  the `prefix_var` of `make()`.
- The build helpers already do this. Check the recipe uses them.

If a build looks for a file in your home (a config file, a credential), it
won't find it: the sandbox hides your home. Pass what it needs explicitly in
the recipe. As a last resort, `mode = "off"` under `[sandbox]` in
`config.toml` disables the sandbox (lesson 9).

## `package ... is not defined`

```
error: package `ghost` is not defined (searched: `./repository`)
```

No repository has a recipe for that package. Check:

- the spelling;
- the `[[repository]]` entries in `Nux.toml` and `config.toml`. The message
  lists the directories Nux searched;
- that the recipe's `program` name matches. The file name doesn't matter.

## `no version of ... satisfies ...`

```
error: no version of `lua` satisfies `^6` (available: 5.3.6, 5.4.7)
```

The requirement matches none of the versions the recipe offers. Change the
requirement, or add the version to the recipe. If the recipe uses the host
(lesson 7), the available versions depend on what is installed on this
machine.

## `cannot link ...: ... already exists`

```
error: cannot link `jq`: `/home/you/.nux/bin/jq` already exists and it is not a link created by nux
```

There is a file in `~/.nux/bin` that Nux didn't create, and Nux never
overwrites files it doesn't own. Move or delete that file, then sync again.
Nux checks this before building anything.

## `the Nux workspace ... is in use by another nux process`

```
error: the Nux workspace `/home/you/.nux` is in use by another nux process (pid 4242); pass `--wait` to wait for it to finish
```

Only one Nux command may change the workspace at a time. Wait for the other
one, or add `--wait` to queue behind it. If the other process was killed,
there is nothing to clean up: the lock is released automatically.

## `Nux.lock is out of date` with `--locked`

```
error: `Nux.lock` is out of date: `zlib` would change from 1.3.1 to 1.2.13; run `nux sync` without `--locked` to update it
```

Someone changed `Nux.toml` (or a recipe) without updating the lock. Run
`nux sync` without `--locked` and commit the new `Nux.lock`.

## `checksum mismatch`

```
error: failed to download zlib 1.3.1: checksum mismatch for `https://...`: Nux.lock expects sha256 9a93..., the download has 3f2b...
```

The file at that URL is not the one the lock recorded. The server changed
it, or something is wrong in between. Nux refuses to build it. If you trust
the new file, delete the package's entry from `Nux.lock` (or the whole file)
and sync again.

## `builds will not be sandboxed`

```
warning: builds will not be sandboxed: sandboxes cannot be created here: ...
```

Your system doesn't allow the sandbox. This is common inside containers, and
on systems that restrict "unprivileged user namespaces". Builds still run,
without the sandbox's protection. To silence the warning, set
`mode = "off"` under `[sandbox]`. To refuse unsandboxed builds instead, set
`mode = "required"`.

## `no answer to the confirmation prompt`

```
error: no answer to the confirmation prompt; pass `--yes` to sync without asking
```

Nux asked a question but nothing could answer, typically in a script or CI.
Add `-y`.

## I changed my source, but Nux says it's up to date

Nux rebuilds when the *recipe* changes, not when files of a `local` source
change. Bump the version in the recipe (lesson 4).

## Something else

- `nux help <command>` explains every option of a command.
- `nux status` shows what Nux sees.
- Build logs are in `~/.nux/packages/<name>/<version>/build.log`.
- When the workspace is in a state you don't understand, `nux reset` starts
  over without losing your environments (lesson 8).
