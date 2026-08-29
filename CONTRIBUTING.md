# Contributing

Bugs and ideas go to the [issue tracker](https://github.com/sachesi/locus/issues);
security problems do not, see [SECURITY.md](SECURITY.md).

Before a change goes in:

- `just check` and `just test` pass. CI runs both on Fedora 44, with `cargo deny check`, for
  every push and pull request.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):
  `fix:`, `feat:`, `perf:`, `docs:` and so on, with a subject that says what changed for
  someone using Locus.
- Every string the user sees goes through `gettext`. `just po` updates the catalogues in
  `po/`, and a change that adds strings brings their translations along where it can.

## Where things are

    build.rs               runs blueprint-compiler and bundles the GResource
    data/ui/*.blp          the window, the two pages and the shortcuts dialog
    data/style.css         structural CSS, colours come from libadwaita
    src/main.rs            the application: its actions, the About dialog, one window
    src/window.rs          the window, its win.* actions and settings, and the loop that
                           takes a reading, hands it to the pages and waits for the next
    src/sampler.rs         a reading of the machine and its processes, on a worker thread,
                           with the counters of the last one to measure rates against
    src/procfs.rs          parsers of the /proc files, pure functions over their text
    src/process.rs         LocusProcess, one process in the list model, and signalling it
    src/processes_page.rs  the process list: columns, sorting, search, the process.* actions
                           and their confirmations
    src/resources_page.rs  the processor, memory, network, disk and file system cards
    src/graph.rs           LocusGraph, a line graph drawn with GtkSnapshot
    src/format.rs          percentages, sizes and rates as the interface writes them

Nothing in `sampler.rs` or `procfs.rs` touches GTK: a `Sampler` moves to a
`gio::spawn_blocking` worker for each reading and comes back with a `Snapshot` of plain
values. One reading runs at a time, and the window waits for the update interval, or for
something that changes what the next reading covers, before it asks for another.

The list is a `GtkColumnView` over a `GListStore` of `LocusProcess`, filtered and sorted in
the Blueprint file. A reading updates the objects of the processes it has seen before, so
the rows showing them keep their place and selection, and only properties that changed are
set. The sort list model does not watch the values it sorts by, so the view's sorter is told
after each reading.

A process is signalled through a pidfd, after checking that the process holding the pid
still has the start time it was listed with: a pid reused since the last reading gets
nothing.

## Running

    just run            # debug build, schema from target/schemas, translations from target/locale
    just check          # fmt, clippy -D warnings, blueprint, validators, catalogues
    just test           # the unit tests
    cargo deny check    # advisories, licences and sources of the dependencies

The tests of the parsers run on fixtures; one reads this machine's `/proc` and finds its own
process there, and one starts `sleep` and signals it.

User-visible strings use `{name}` placeholders filled in with `str::replace`; a string in a
`gettext` call stays on one line, since xgettext reads Rust's line continuations as spaces
the program never sees. `just pot` regenerates the template from the Rust sources, the
Blueprint files, the desktop entry, the metainfo and the schema, and `just po` merges it into
every `po/<lang>.po`. A new language is a new line in `po/LINGUAS` plus the `.po` file.
`install` compiles the catalogues and merges the desktop and metainfo translations with
`msgfmt`.
